import { defineStore } from 'pinia';

import { LOG_DOMAINS, LOG_EVENTS } from '@/lib/models/telemetry';
import {
  parseChatEventEnvelope,
  type ChatAgentEvent,
  type ChatAgentInfo,
  type ChatMessageEntry,
  type ChatPendingPermission,
  type ChatPlanEntry,
  type ChatRunFailure,
  type ChatSessionPhase,
  type ChatTimelineEntry,
  type ChatToolCallEntry,
} from '@/lib/models/chat';
import { ChatService } from '@/lib/services/chat-service';
import { LoggerService } from '@/lib/services/logger-service';
import { parseCommandError } from '@/lib/utils/error';

/** Cap on rendered tool-call argument JSON so a bulk tool input cannot flood the timeline. */
const TOOL_ARGS_DISPLAY_MAX_CHARS = 4000;

interface ChatState {
  providers: ChatAgentInfo[];
  probing: boolean;
  /** True after the first probe answered; an empty provider list is only shown then. */
  probed: boolean;
  selectedProviderId: string | null;
  /** Requested for the next session; the active session reports its own flag. */
  mutationsEnabled: boolean;
  sessionId: string | null;
  sessionPhase: ChatSessionPhase;
  providerDisplayName: string | null;
  sessionMutationsEnabled: boolean;
  entries: ChatTimelineEntry[];
  planEntries: ChatPlanEntry[];
  pendingPermissions: ChatPendingPermission[];
  activeRunId: string | null;
  runError: ChatRunFailure | null;
  /** Typed start failure: an agent error code, an adapter reason, or the command code. */
  startErrorCode: string | null;
  sending: boolean;
  lastEventSeq: number | null;
  sessionUnlistener: (() => void) | null;
}

function formatToolArgs(args: unknown): string {
  try {
    const json = JSON.stringify(args, null, 2) ?? '';
    return json.length > TOOL_ARGS_DISPLAY_MAX_CHARS ? `${json.slice(0, TOOL_ARGS_DISPLAY_MAX_CHARS)}…` : json;
  } catch {
    return '';
  }
}

function findMessage(entries: ChatTimelineEntry[], messageId: string): ChatMessageEntry | null {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index];
    if (entry.kind === 'message' && entry.id === messageId) return entry;
  }
  return null;
}

function findToolCall(entries: ChatTimelineEntry[], toolCallId: string): ChatToolCallEntry | null {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index];
    if (entry.kind === 'tool' && entry.id === toolCallId) return entry;
  }
  return null;
}

/** Extracts the typed chat failure from a command error envelope. */
function chatCommandErrorCode(error: unknown): string {
  const commandError = parseCommandError(error);
  if (!commandError) return 'operationFailed';
  return commandError.details.agentError ?? commandError.details.reason ?? commandError.code;
}

export const useChatStore = defineStore('chat', {
  state: (): ChatState => ({
    providers: [],
    probing: false,
    probed: false,
    selectedProviderId: null,
    mutationsEnabled: false,
    sessionId: null,
    sessionPhase: 'idle',
    providerDisplayName: null,
    sessionMutationsEnabled: false,
    entries: [],
    planEntries: [],
    pendingPermissions: [],
    activeRunId: null,
    runError: null,
    startErrorCode: null,
    sending: false,
    lastEventSeq: null,
    sessionUnlistener: null,
  }),
  getters: {
    sessionActive: state => state.sessionPhase === 'active',
    /** A run is in flight: the composer offers Stop instead of Send. */
    runInFlight: state => state.sessionPhase === 'active' && state.activeRunId !== null,
    canSend(): boolean {
      return this.sessionActive && !this.runInFlight && !this.sending;
    },
    selectedProvider(): ChatAgentInfo | null {
      return this.providers.find(provider => provider.id === this.selectedProviderId) ?? null;
    },
  },
  actions: {
    /** Probes installed provider CLIs; called lazily when the chat page opens. */
    async probeAgents() {
      if (this.probing) return;
      this.probing = true;
      try {
        this.providers = await ChatService.probeAgents();
        if (!this.providers.some(provider => provider.id === this.selectedProviderId)) {
          this.selectedProviderId = this.providers[0]?.id ?? null;
        }
      } catch (error) {
        // A failed probe fails closed to the same empty state as "no provider
        // installed"; the page offers an explicit retry.
        this.providers = [];
        this.selectedProviderId = null;
        LoggerService.warn(LOG_DOMAINS.chat, LOG_EVENTS.chatProviderProbeFailed, { error });
      } finally {
        this.probing = false;
        this.probed = true;
      }
    },
    selectProvider(providerId: string) {
      if (this.sessionPhase === 'active' || this.sessionPhase === 'starting') return;
      if (this.providers.some(provider => provider.id === providerId)) {
        this.selectedProviderId = providerId;
      }
    },
    setMutationsEnabled(enabled: boolean) {
      this.mutationsEnabled = enabled;
    },
    async startSession() {
      if (!this.selectedProviderId) return;
      if (this.sessionPhase === 'starting' || this.sessionPhase === 'active') return;
      // A previous ended session still holds its provider process handle;
      // closing is idempotent and best-effort before the fresh start.
      await this.closeSession();
      this.sessionPhase = 'starting';
      this.startErrorCode = null;
      this.resetTranscript();
      try {
        const info = await ChatService.startSession(this.selectedProviderId, this.mutationsEnabled);
        const unlistenEvents = await ChatService.listenSessionEvents(info.sessionId, payload => {
          this.handleRawEvent(payload);
        });
        const unlistenEnded = await ChatService.listenSessionEnded(info.sessionId, () => {
          this.handleSessionEnded();
        });
        this.sessionUnlistener = () => {
          unlistenEvents();
          unlistenEnded();
        };
        this.sessionId = info.sessionId;
        this.providerDisplayName = info.providerDisplayName;
        this.sessionMutationsEnabled = info.mutationsEnabled;
        this.sessionPhase = 'active';
      } catch (error) {
        this.sessionPhase = 'idle';
        this.startErrorCode = chatCommandErrorCode(error);
        LoggerService.warn(LOG_DOMAINS.chat, LOG_EVENTS.chatSessionStartFailed, { code: this.startErrorCode });
      }
    },
    async closeSession() {
      const sessionId = this.sessionId;
      this.sessionUnlistener?.();
      this.sessionUnlistener = null;
      this.sessionId = null;
      this.sessionPhase = 'idle';
      if (!sessionId) return;
      try {
        await ChatService.closeSession(sessionId);
      } catch {
        // Closing is best-effort: the provider may already be gone, and the
        // bridge reaps the process either way.
      }
    },
    async sendPrompt(text: string) {
      const trimmed = text.trim();
      if (!trimmed || !this.canSend || !this.sessionId) return;
      this.entries.push({
        kind: 'message',
        id: `user-${crypto.randomUUID()}`,
        role: 'user',
        thought: false,
        text: trimmed,
        streaming: false,
      });
      this.sending = true;
      this.runError = null;
      try {
        // The RUN_STARTED event confirms the run; the returned id correlates
        // the composer state without waiting for the event round-trip.
        this.activeRunId = await ChatService.sendPrompt(this.sessionId, trimmed);
      } catch (error) {
        const code = chatCommandErrorCode(error);
        this.runError = { runId: null, code, message: '' };
        LoggerService.warn(LOG_DOMAINS.chat, LOG_EVENTS.chatPromptFailed, { code });
      } finally {
        this.sending = false;
      }
    },
    async cancelRun() {
      if (!this.runInFlight || !this.sessionId) return;
      try {
        await ChatService.cancel(this.sessionId);
      } catch (error) {
        LoggerService.warn(LOG_DOMAINS.chat, LOG_EVENTS.chatPromptFailed, {
          code: chatCommandErrorCode(error),
        });
      }
    },
    /** Answers a pending permission prompt; a null optionId declines it. */
    async resolvePermission(requestId: number, optionId: string | null) {
      const sessionId = this.sessionId;
      this.pendingPermissions = this.pendingPermissions.filter(request => request.requestId !== requestId);
      if (!sessionId || this.sessionPhase !== 'active') return;
      try {
        await ChatService.resolvePermission(sessionId, requestId, optionId);
      } catch {
        // The bridge auto-declines timed-out prompts, so an already-resolved
        // answer racing the click is expected and needs no UI recovery.
      }
    },
    /** Event-channel entry point; validates the envelope before applying it. */
    handleRawEvent(payload: unknown) {
      const envelope = parseChatEventEnvelope(payload);
      if (!envelope) {
        LoggerService.warn(LOG_DOMAINS.chat, LOG_EVENTS.chatSessionEventDropped);
        return;
      }
      if (this.lastEventSeq !== null && envelope.seq !== this.lastEventSeq + 1) {
        // A gap means events were dropped between bridge and store; the
        // transcript stays consistent but the diagnostic keeps it visible.
        LoggerService.warn(LOG_DOMAINS.chat, LOG_EVENTS.chatEventSequenceGap, {
          expectedSeq: this.lastEventSeq + 1,
          receivedSeq: envelope.seq,
        });
      }
      this.lastEventSeq = envelope.seq;
      this.applyEvent(envelope.event);
    },
    handleSessionEnded() {
      if (this.sessionPhase !== 'active') return;
      this.sessionPhase = 'ended';
      this.activeRunId = null;
      this.pendingPermissions = [];
      this.settleStreamingEntries();
      this.sessionUnlistener?.();
      this.sessionUnlistener = null;
      LoggerService.info(LOG_DOMAINS.chat, LOG_EVENTS.chatSessionEnded);
    },
    /** Applies one validated session event to the timeline state machine. */
    applyEvent(event: ChatAgentEvent) {
      switch (event.type) {
        case 'RUN_STARTED':
          this.activeRunId = event.run_id;
          this.runError = null;
          break;
        case 'RUN_FINISHED':
          this.activeRunId = null;
          this.settleStreamingEntries();
          break;
        case 'RUN_ERROR':
          this.activeRunId = null;
          this.settleStreamingEntries();
          this.runError = { runId: event.run_id, code: event.code, message: event.message };
          break;
        case 'TEXT_MESSAGE_START':
          this.entries.push(this.createAgentMessage(event.message_id, false));
          break;
        case 'TEXT_MESSAGE_CONTENT':
          this.appendMessageDelta(event.message_id, false, event.delta);
          break;
        case 'TEXT_MESSAGE_END':
          this.endMessageStream(event.message_id);
          break;
        case 'THINKING_MESSAGE_START':
          this.entries.push(this.createAgentMessage(event.message_id, true));
          break;
        case 'THINKING_MESSAGE_CONTENT':
          this.appendMessageDelta(event.message_id, true, event.delta);
          break;
        case 'THINKING_MESSAGE_END':
          this.endMessageStream(event.message_id);
          break;
        case 'TOOL_CALL_START':
          this.entries.push({
            kind: 'tool',
            id: event.tool_call_id,
            name: event.name,
            toolKind: event.kind,
            status: event.status,
            argsJson: null,
          });
          break;
        case 'TOOL_CALL_ARGS': {
          const toolCall = findToolCall(this.entries, event.tool_call_id);
          if (toolCall) toolCall.argsJson = formatToolArgs(event.args);
          break;
        }
        case 'TOOL_CALL_END': {
          const toolCall = findToolCall(this.entries, event.tool_call_id);
          if (toolCall) toolCall.status = event.status;
          break;
        }
        case 'PLAN_UPDATED':
          this.planEntries = event.entries;
          break;
        case 'PERMISSION_REQUESTED':
          this.pendingPermissions.push({
            requestId: event.request_id,
            toolCallId: event.tool_call_id,
            title: event.title,
            options: event.options,
          });
          break;
      }
    },
    createAgentMessage(messageId: string, thought: boolean): ChatMessageEntry {
      return { kind: 'message', id: messageId, role: 'agent', thought, text: '', streaming: true };
    },
    appendMessageDelta(messageId: string, thought: boolean, delta: string) {
      // The start event can be lost to a channel gap; recreating the message
      // keeps the content instead of dropping the whole stream.
      let message = findMessage(this.entries, messageId);
      if (!message) {
        this.entries.push(this.createAgentMessage(messageId, thought));
        message = findMessage(this.entries, messageId);
      }
      if (message) message.text += delta;
    },
    endMessageStream(messageId: string) {
      const message = findMessage(this.entries, messageId);
      if (message) message.streaming = false;
    },
    /** Marks every still-streaming message settled when a run ends or errors. */
    settleStreamingEntries() {
      for (const entry of this.entries) {
        if (entry.kind === 'message' && entry.streaming) entry.streaming = false;
      }
    },
    resetTranscript() {
      this.entries = [];
      this.planEntries = [];
      this.pendingPermissions = [];
      this.activeRunId = null;
      this.runError = null;
      this.lastEventSeq = null;
    },
  },
});
