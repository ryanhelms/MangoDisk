import { createPinia, setActivePinia } from 'pinia';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { ChatAgentInfo, ChatSessionInfo } from '@/lib/models/chat';
import { ChatService } from '@/lib/services/chat-service';
import { LoggerService } from '@/lib/services/logger-service';

import { useChatStore } from './chat-store';

const claude: ChatAgentInfo = { id: 'claude', displayName: 'Claude Code', version: '2.1.0' };
const codex: ChatAgentInfo = { id: 'codex', displayName: 'OpenAI Codex', version: null };

function sessionInfo(overrides: Partial<ChatSessionInfo> = {}): ChatSessionInfo {
  return {
    sessionId: 'chat-1',
    providerId: 'claude',
    providerDisplayName: 'Claude Code',
    mutationsEnabled: false,
    ...overrides,
  };
}

/** Drives the raw channel entry point the way the Tauri listener would. */
function emitRaw(store: ReturnType<typeof useChatStore>, seq: number, event: Record<string, unknown>) {
  store.handleRawEvent({ schema_version: 1, seq, ...event });
}

describe('chat store provider probing', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.restoreAllMocks();
  });

  it('keeps only probed providers and selects the first one', async () => {
    vi.spyOn(ChatService, 'probeAgents').mockResolvedValue([claude, codex]);
    const store = useChatStore();

    await store.probeAgents();

    expect(store.providers).toEqual([claude, codex]);
    expect(store.probed).toBe(true);
    expect(store.selectedProviderId).toBe('claude');
  });

  it('fails closed to the empty state when the probe command fails', async () => {
    vi.spyOn(ChatService, 'probeAgents').mockRejectedValue(new Error('invoke unavailable'));
    const warn = vi.spyOn(LoggerService, 'warn').mockImplementation(() => undefined);
    const store = useChatStore();

    await store.probeAgents();

    expect(store.providers).toEqual([]);
    expect(store.selectedProviderId).toBeNull();
    expect(store.probed).toBe(true);
    expect(warn).toHaveBeenCalledWith('chat', 'chat_provider_probe_failed', expect.anything());
  });
});

describe('chat store session lifecycle', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.restoreAllMocks();
  });

  async function startActiveSession(store: ReturnType<typeof useChatStore>) {
    const unlisten = vi.fn();
    vi.spyOn(ChatService, 'startSession').mockResolvedValue(sessionInfo());
    vi.spyOn(ChatService, 'listenSessionEvents').mockResolvedValue(unlisten);
    vi.spyOn(ChatService, 'listenSessionEnded').mockResolvedValue(vi.fn());
    vi.spyOn(ChatService, 'closeSession').mockResolvedValue();
    store.providers = [claude];
    store.selectedProviderId = 'claude';

    await store.startSession();
    return { unlisten };
  }

  it('starts a session, subscribes to its channels, and exposes metadata', async () => {
    const store = useChatStore();
    const { unlisten } = await startActiveSession(store);

    expect(ChatService.startSession).toHaveBeenCalledWith('claude', false);
    expect(ChatService.listenSessionEvents).toHaveBeenCalledWith('chat-1', expect.any(Function));
    expect(store.sessionPhase).toBe('active');
    expect(store.sessionId).toBe('chat-1');
    expect(store.providerDisplayName).toBe('Claude Code');
    expect(unlisten).not.toHaveBeenCalled();
  });

  it('passes the mutations opt-in through to the session start', async () => {
    const store = useChatStore();
    store.mutationsEnabled = true;
    await startActiveSession(store);

    expect(ChatService.startSession).toHaveBeenCalledWith('claude', true);
  });

  it('surfaces a typed start failure without leaving a half-open session', async () => {
    vi.spyOn(ChatService, 'startSession').mockRejectedValue({
      code: 'operationFailed',
      details: { operation: 'chat_start_session', agentError: 'provider_unavailable' },
      retryable: true,
    });
    vi.spyOn(LoggerService, 'warn').mockImplementation(() => undefined);
    const store = useChatStore();
    store.providers = [claude];
    store.selectedProviderId = 'claude';

    await store.startSession();

    expect(store.sessionPhase).toBe('idle');
    expect(store.sessionId).toBeNull();
    expect(store.startErrorCode).toBe('provider_unavailable');
  });

  it('closes idempotently and releases the session channels', async () => {
    const store = useChatStore();
    const { unlisten } = await startActiveSession(store);

    await store.closeSession();

    expect(unlisten).toHaveBeenCalled();
    expect(ChatService.closeSession).toHaveBeenCalledWith('chat-1');
    expect(store.sessionPhase).toBe('idle');
    expect(store.sessionId).toBeNull();

    await store.closeSession();
    expect(ChatService.closeSession).toHaveBeenCalledTimes(1);
  });
});

describe('chat store event state machine', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.restoreAllMocks();
  });

  function activeStore() {
    const store = useChatStore();
    store.sessionId = 'chat-1';
    store.sessionPhase = 'active';
    return store;
  }

  it('accumulates streaming text and settles the message at its end', () => {
    const store = activeStore();

    emitRaw(store, 0, { type: 'TEXT_MESSAGE_START', message_id: 'm-1' });
    emitRaw(store, 1, { type: 'TEXT_MESSAGE_CONTENT', message_id: 'm-1', delta: 'Disk ' });
    emitRaw(store, 2, { type: 'TEXT_MESSAGE_CONTENT', message_id: 'm-1', delta: 'usage' });
    emitRaw(store, 3, { type: 'TEXT_MESSAGE_END', message_id: 'm-1' });

    expect(store.entries).toEqual([
      { kind: 'message', id: 'm-1', role: 'agent', thought: false, text: 'Disk usage', streaming: false },
    ]);
  });

  it('keeps reasoning streams in separate thought messages', () => {
    const store = activeStore();

    emitRaw(store, 0, { type: 'THINKING_MESSAGE_START', message_id: 't-1' });
    emitRaw(store, 1, { type: 'THINKING_MESSAGE_CONTENT', message_id: 't-1', delta: 'checking' });
    emitRaw(store, 2, { type: 'THINKING_MESSAGE_END', message_id: 't-1' });

    expect(store.entries).toEqual([
      { kind: 'message', id: 't-1', role: 'agent', thought: true, text: 'checking', streaming: false },
    ]);
  });

  it('recreates a message whose start event was lost instead of dropping content', () => {
    const store = activeStore();

    emitRaw(store, 4, { type: 'TEXT_MESSAGE_CONTENT', message_id: 'm-late', delta: 'partial' });

    expect(store.entries).toEqual([
      { kind: 'message', id: 'm-late', role: 'agent', thought: false, text: 'partial', streaming: true },
    ]);
  });

  it('tracks tool calls from start through args to terminal status', () => {
    const store = activeStore();

    emitRaw(store, 0, {
      type: 'TOOL_CALL_START',
      tool_call_id: 'call-1',
      name: 'mangodisk: scan',
      kind: 'search',
      status: 'in_progress',
    });
    emitRaw(store, 1, { type: 'TOOL_CALL_ARGS', tool_call_id: 'call-1', args: { limit: 3 } });
    emitRaw(store, 2, { type: 'TOOL_CALL_END', tool_call_id: 'call-1', status: 'completed' });

    expect(store.entries).toEqual([
      {
        kind: 'tool',
        id: 'call-1',
        name: 'mangodisk: scan',
        toolKind: 'search',
        status: 'completed',
        argsJson: '{\n  "limit": 3\n}',
      },
    ]);
  });

  it('routes the run lifecycle and settles streaming entries on finish', () => {
    const store = activeStore();

    emitRaw(store, 0, { type: 'RUN_STARTED', run_id: 'run-1' });
    expect(store.activeRunId).toBe('run-1');
    expect(store.runInFlight).toBe(true);

    emitRaw(store, 1, { type: 'TEXT_MESSAGE_START', message_id: 'm-1' });
    emitRaw(store, 2, { type: 'RUN_FINISHED', run_id: 'run-1', stop_reason: 'end_turn' });

    expect(store.activeRunId).toBeNull();
    expect(store.runInFlight).toBe(false);
    const message = store.entries[0];
    expect(message.kind === 'message' && message.streaming).toBe(false);
  });

  it('records run errors with their typed code', () => {
    const store = activeStore();

    emitRaw(store, 0, { type: 'RUN_STARTED', run_id: 'run-2' });
    emitRaw(store, 1, { type: 'RUN_ERROR', run_id: 'run-2', code: 'prompt_failed', message: 'agent said no' });

    expect(store.activeRunId).toBeNull();
    expect(store.runError).toEqual({ runId: 'run-2', code: 'prompt_failed', message: 'agent said no' });
  });

  it('replaces the plan snapshot on every update', () => {
    const store = activeStore();

    emitRaw(store, 0, {
      type: 'PLAN_UPDATED',
      entries: [{ content: 'Scan caches', priority: 'high', status: 'in_progress' }],
    });
    emitRaw(store, 1, {
      type: 'PLAN_UPDATED',
      entries: [{ content: 'Scan caches', priority: 'high', status: 'completed' }],
    });

    expect(store.planEntries).toEqual([{ content: 'Scan caches', priority: 'high', status: 'completed' }]);
  });

  it('warns when the event sequence has a gap', () => {
    const warn = vi.spyOn(LoggerService, 'warn').mockImplementation(() => undefined);
    const store = activeStore();

    emitRaw(store, 0, { type: 'RUN_STARTED', run_id: 'run-1' });
    emitRaw(store, 3, { type: 'RUN_FINISHED', run_id: 'run-1', stop_reason: 'end_turn' });

    expect(warn).toHaveBeenCalledWith('chat', 'chat_event_sequence_gap', { expectedSeq: 1, receivedSeq: 3 });
    expect(store.lastEventSeq).toBe(3);
  });

  it('drops envelopes from an unknown schema version', () => {
    const warn = vi.spyOn(LoggerService, 'warn').mockImplementation(() => undefined);
    const store = activeStore();

    store.handleRawEvent({ schema_version: 99, seq: 0, type: 'RUN_STARTED', run_id: 'run-x' });

    expect(store.activeRunId).toBeNull();
    expect(warn).toHaveBeenCalledWith('chat', 'chat_session_event_dropped');
  });

  it('ends the session terminally when the stream closes', () => {
    const store = activeStore();
    store.activeRunId = 'run-1';
    store.pendingPermissions = [{ requestId: 1, toolCallId: 'call-1', title: null, options: [] }];

    store.handleSessionEnded();

    expect(store.sessionPhase).toBe('ended');
    expect(store.activeRunId).toBeNull();
    expect(store.pendingPermissions).toEqual([]);
    expect(store.canSend).toBe(false);
  });
});

describe('chat store permission flow', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.restoreAllMocks();
  });

  function storeWithPermission() {
    const store = useChatStore();
    store.sessionId = 'chat-1';
    store.sessionPhase = 'active';
    store.applyEvent({
      type: 'PERMISSION_REQUESTED',
      request_id: 7,
      tool_call_id: 'call-1',
      title: 'Delete selected cache files',
      options: [
        { option_id: 'allow-once', label: 'Allow once', kind: 'allow_once' },
        { option_id: 'reject-once', label: 'Reject', kind: 'reject_once' },
      ],
    });
    return store;
  }

  it('queues permission requests from events', () => {
    const store = storeWithPermission();

    expect(store.pendingPermissions).toEqual([
      {
        requestId: 7,
        toolCallId: 'call-1',
        title: 'Delete selected cache files',
        options: [
          { option_id: 'allow-once', label: 'Allow once', kind: 'allow_once' },
          { option_id: 'reject-once', label: 'Reject', kind: 'reject_once' },
        ],
      },
    ]);
  });

  it('resolves a permission with the selected option and clears it', async () => {
    const resolve = vi.spyOn(ChatService, 'resolvePermission').mockResolvedValue();
    const store = storeWithPermission();

    await store.resolvePermission(7, 'allow-once');

    expect(store.pendingPermissions).toEqual([]);
    expect(resolve).toHaveBeenCalledWith('chat-1', 7, 'allow-once');
  });

  it('declines a permission with a null option', async () => {
    const resolve = vi.spyOn(ChatService, 'resolvePermission').mockResolvedValue();
    const store = storeWithPermission();

    await store.resolvePermission(7, null);

    expect(store.pendingPermissions).toEqual([]);
    expect(resolve).toHaveBeenCalledWith('chat-1', 7, null);
  });

  it('tolerates answers that arrive after the request expired', async () => {
    vi.spyOn(ChatService, 'resolvePermission').mockRejectedValue({
      code: 'invalidInput',
      details: { operation: 'chat_resolve_permission', agentError: 'permission_not_pending' },
      retryable: false,
    });
    const store = storeWithPermission();

    await store.resolvePermission(7, 'allow-once');

    expect(store.pendingPermissions).toEqual([]);
  });
});

describe('chat store prompting', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.restoreAllMocks();
  });

  it('appends the user message and activates the returned run', async () => {
    vi.spyOn(ChatService, 'sendPrompt').mockResolvedValue('run-9');
    const store = useChatStore();
    store.sessionId = 'chat-1';
    store.sessionPhase = 'active';

    await store.sendPrompt('  what can I delete?  ');

    expect(ChatService.sendPrompt).toHaveBeenCalledWith('chat-1', 'what can I delete?');
    expect(store.activeRunId).toBe('run-9');
    expect(store.entries).toHaveLength(1);
    expect(store.entries[0]).toMatchObject({ kind: 'message', role: 'user', text: 'what can I delete?' });
  });

  it('refuses prompts while a run is in flight or the text is empty', async () => {
    const send = vi.spyOn(ChatService, 'sendPrompt');
    const store = useChatStore();
    store.sessionId = 'chat-1';
    store.sessionPhase = 'active';

    await store.sendPrompt('   ');
    expect(send).not.toHaveBeenCalled();

    store.activeRunId = 'run-1';
    await store.sendPrompt('second');
    expect(send).not.toHaveBeenCalled();
  });

  it('turns a failed prompt invoke into a run failure banner', async () => {
    vi.spyOn(ChatService, 'sendPrompt').mockRejectedValue({
      code: 'operationFailed',
      details: { operation: 'chat_send_prompt', agentError: 'session_lost' },
      retryable: true,
    });
    vi.spyOn(LoggerService, 'warn').mockImplementation(() => undefined);
    const store = useChatStore();
    store.sessionId = 'chat-1';
    store.sessionPhase = 'active';

    await store.sendPrompt('hello');

    expect(store.runError).toEqual({ runId: null, code: 'session_lost', message: '' });
    expect(store.sending).toBe(false);
  });
});
