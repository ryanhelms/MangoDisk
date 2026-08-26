import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import {
  chatSessionEndedEventName,
  chatSessionEventName,
  type ChatAgentInfo,
  type ChatSessionEndedEvent,
  type ChatSessionInfo,
} from '@/lib/models/chat';

export class ChatService {
  static probeAgents(): Promise<ChatAgentInfo[]> {
    return invoke<ChatAgentInfo[]>('chat_probe_agents');
  }

  static startSession(providerId: string, enableMutations: boolean): Promise<ChatSessionInfo> {
    return invoke<ChatSessionInfo>('chat_start_session', { providerId, enableMutations });
  }

  /** Returns the bridge-assigned run id; run progress arrives as events. */
  static sendPrompt(sessionId: string, text: string): Promise<string> {
    return invoke<string>('chat_send_prompt', { sessionId, text });
  }

  static cancel(sessionId: string): Promise<void> {
    return invoke<void>('chat_cancel', { sessionId });
  }

  /** A null optionId declines the permission request. */
  static resolvePermission(sessionId: string, requestId: number, optionId: string | null): Promise<void> {
    return invoke<void>('chat_resolve_permission', { sessionId, requestId, optionId });
  }

  static closeSession(sessionId: string): Promise<void> {
    return invoke<void>('chat_close_session', { sessionId });
  }

  /** Raw channel payloads; consumers parse them with parseChatEventEnvelope. */
  static listenSessionEvents(sessionId: string, handler: (payload: unknown) => void): Promise<UnlistenFn> {
    return listen<unknown>(chatSessionEventName(sessionId), event => handler(event.payload));
  }

  static listenSessionEnded(sessionId: string, handler: (event: ChatSessionEndedEvent) => void): Promise<UnlistenFn> {
    return listen<ChatSessionEndedEvent>(chatSessionEndedEventName(sessionId), event => handler(event.payload));
  }
}
