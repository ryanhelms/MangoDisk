import { ICON_NAMES } from './ui';

/**
 * Chat protocol between the desktop adapter and this frontend. The wire
 * shapes mirror `mangodisk-acp`'s AG-UI event vocabulary: serde flattening
 * places the event fields next to the envelope metadata, event `type` tags
 * are SCREAMING_SNAKE_CASE, and field names stay snake_case. The maps of
 * locale keys exist so enum-driven labels keep literal, checkable translation
 * consumers instead of template-built key paths.
 */

export interface ChatAgentInfo {
  id: string;
  displayName: string;
  version: string | null;
}

export interface ChatSessionInfo {
  sessionId: string;
  providerId: string;
  providerDisplayName: string;
  mutationsEnabled: boolean;
}

/** Stable ACP bridge error codes, serialized snake_case by the adapter. */
export type ChatAgentErrorCode =
  | 'provider_unknown'
  | 'provider_unavailable'
  | 'spawn_failed'
  | 'handshake_failed'
  | 'session_lost'
  | 'provider_exited'
  | 'prompt_failed'
  | 'timeout'
  | 'permission_not_pending';

/** Adapter-local failure reasons carried in the command error details. */
export type ChatAdapterFailureReason = 'agentSidecarUnavailable' | 'chatSessionUnknown' | 'promptEmpty';

export type ChatRunStopReason = 'end_turn' | 'max_tokens' | 'max_turn_requests' | 'refusal' | 'cancelled' | 'other';

export type ChatToolCallKind =
  'read' | 'edit' | 'delete' | 'move' | 'search' | 'execute' | 'think' | 'fetch' | 'switch_mode' | 'other';

export type ChatToolCallStatus = 'pending' | 'in_progress' | 'completed' | 'failed' | 'other';

export type ChatPermissionOptionKind = 'allow_once' | 'allow_always' | 'reject_once' | 'reject_always' | 'other';

export type ChatPlanEntryPriority = 'high' | 'medium' | 'low' | 'other';
export type ChatPlanEntryStatus = 'pending' | 'in_progress' | 'completed' | 'other';

export interface ChatPlanEntry {
  content: string;
  priority: ChatPlanEntryPriority;
  status: ChatPlanEntryStatus;
}

export interface ChatPermissionOption {
  option_id: string;
  label: string;
  kind: ChatPermissionOptionKind;
}

export type ChatAgentEvent =
  | { type: 'RUN_STARTED'; run_id: string }
  | { type: 'RUN_FINISHED'; run_id: string; stop_reason: ChatRunStopReason }
  | { type: 'RUN_ERROR'; run_id: string | null; code: ChatAgentErrorCode; message: string }
  | { type: 'TEXT_MESSAGE_START'; message_id: string }
  | { type: 'TEXT_MESSAGE_CONTENT'; message_id: string; delta: string }
  | { type: 'TEXT_MESSAGE_END'; message_id: string }
  | { type: 'THINKING_MESSAGE_START'; message_id: string }
  | { type: 'THINKING_MESSAGE_CONTENT'; message_id: string; delta: string }
  | { type: 'THINKING_MESSAGE_END'; message_id: string }
  | {
      type: 'TOOL_CALL_START';
      tool_call_id: string;
      name: string;
      kind: ChatToolCallKind;
      status: ChatToolCallStatus;
    }
  | { type: 'TOOL_CALL_ARGS'; tool_call_id: string; args: unknown }
  | { type: 'TOOL_CALL_END'; tool_call_id: string; status: ChatToolCallStatus }
  | { type: 'PLAN_UPDATED'; entries: ChatPlanEntry[] }
  | {
      type: 'PERMISSION_REQUESTED';
      request_id: number;
      tool_call_id: string;
      title: string | null;
      options: ChatPermissionOption[];
    };

/** Event stream version understood by this frontend. */
export const CHAT_EVENT_SCHEMA_VERSION = 1 as const;

export interface ChatEventEnvelope {
  schema_version: number;
  seq: number;
  event: ChatAgentEvent;
}

/**
 * Normalizes one raw channel payload. The adapter flattens the envelope
 * (event fields sit next to `schema_version`/`seq`), so this re-nests the
 * event for safer narrowing and rejects payloads from another schema version.
 */
export function parseChatEventEnvelope(payload: unknown): ChatEventEnvelope | null {
  if (typeof payload !== 'object' || payload === null) return null;
  const candidate = payload as Record<string, unknown>;
  if (candidate.schema_version !== CHAT_EVENT_SCHEMA_VERSION) return null;
  if (typeof candidate.seq !== 'number' || typeof candidate.type !== 'string') return null;
  const { schema_version, seq, type, ...fields } = candidate;
  return {
    schema_version,
    seq,
    event: { type, ...fields } as unknown as ChatAgentEvent,
  };
}

export interface ChatSessionEndedEvent {
  sessionId: string;
}

export const CHAT_SESSION_EVENT_PREFIX = 'chat-session-event-' as const;
export const CHAT_SESSION_ENDED_PREFIX = 'chat-session-ended-' as const;

export function chatSessionEventName(sessionId: string): string {
  return `${CHAT_SESSION_EVENT_PREFIX}${sessionId}`;
}

export function chatSessionEndedEventName(sessionId: string): string {
  return `${CHAT_SESSION_ENDED_PREFIX}${sessionId}`;
}

/* UI-side session and timeline shapes. */

export type ChatSessionPhase = 'idle' | 'starting' | 'active' | 'ended';

export interface ChatMessageEntry {
  kind: 'message';
  id: string;
  role: 'user' | 'agent';
  thought: boolean;
  text: string;
  streaming: boolean;
}

export interface ChatToolCallEntry {
  kind: 'tool';
  id: string;
  name: string;
  toolKind: ChatToolCallKind;
  status: ChatToolCallStatus;
  /** Pretty-printed argument JSON when the agent reported raw input. */
  argsJson: string | null;
}

export type ChatTimelineEntry = ChatMessageEntry | ChatToolCallEntry;

export interface ChatPendingPermission {
  requestId: number;
  toolCallId: string;
  title: string | null;
  options: ChatPermissionOption[];
}

export interface ChatRunFailure {
  runId: string | null;
  /**
   * Typed failure identity: a bridge error code for event-sourced failures,
   * or the command/agent error code string when the prompt call itself fails.
   * Pages resolve it through CHAT_AGENT_ERROR_MESSAGE_KEYS with a fallback.
   */
  code: string;
  message: string;
}

/** Locale key per tool-call kind; literal values keep translations checkable. */
export const CHAT_TOOL_KIND_LABEL_KEYS: Record<ChatToolCallKind, string> = {
  read: 'chat.toolKinds.read',
  edit: 'chat.toolKinds.edit',
  delete: 'chat.toolKinds.delete',
  move: 'chat.toolKinds.move',
  search: 'chat.toolKinds.search',
  execute: 'chat.toolKinds.execute',
  think: 'chat.toolKinds.think',
  fetch: 'chat.toolKinds.fetch',
  switch_mode: 'chat.toolKinds.switchMode',
  other: 'chat.toolKinds.other',
};

/** Locale key per tool-call terminal or live status. */
export const CHAT_TOOL_STATUS_LABEL_KEYS: Record<ChatToolCallStatus, string> = {
  pending: 'chat.toolStatuses.pending',
  in_progress: 'chat.toolStatuses.inProgress',
  completed: 'chat.toolStatuses.completed',
  failed: 'chat.toolStatuses.failed',
  other: 'chat.toolStatuses.other',
};

/** Locale key per bridge error code surfaced while a session is active. */
export const CHAT_AGENT_ERROR_MESSAGE_KEYS: Record<ChatAgentErrorCode, string> = {
  provider_unknown: 'chat.errors.providerUnknown',
  provider_unavailable: 'chat.errors.providerUnavailable',
  spawn_failed: 'chat.errors.spawnFailed',
  handshake_failed: 'chat.errors.handshakeFailed',
  session_lost: 'chat.errors.sessionLost',
  provider_exited: 'chat.errors.providerExited',
  prompt_failed: 'chat.errors.promptFailed',
  timeout: 'chat.errors.timeout',
  permission_not_pending: 'chat.errors.permissionNotPending',
};

/** Locale key per adapter-local failure reason. */
export const CHAT_ADAPTER_FAILURE_MESSAGE_KEYS: Record<ChatAdapterFailureReason, string> = {
  agentSidecarUnavailable: 'chat.errors.sidecarUnavailable',
  chatSessionUnknown: 'chat.errors.sessionUnknown',
  promptEmpty: 'chat.errors.promptEmpty',
};

/** Icon per tool-call kind, used by the timeline tool-call card. */
export const CHAT_TOOL_KIND_ICONS: Record<ChatToolCallKind, (typeof ICON_NAMES)[keyof typeof ICON_NAMES]> = {
  read: ICON_NAMES.fileText,
  edit: ICON_NAMES.fileSettings,
  delete: ICON_NAMES.trash,
  move: ICON_NAMES.folder,
  search: ICON_NAMES.search,
  execute: ICON_NAMES.code,
  think: ICON_NAMES.aiTools,
  fetch: ICON_NAMES.globe,
  switch_mode: ICON_NAMES.settings,
  other: ICON_NAMES.package,
};
