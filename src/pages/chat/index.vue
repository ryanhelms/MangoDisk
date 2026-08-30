<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';

import MdEmptyState from '@/components/custom/md-empty-state.vue';
import MdPageShell from '@/components/custom/md-page-shell.vue';
import MdSafeRichText from '@/components/custom/md-safe-rich-text.vue';
import MdSwitch from '@/components/custom/md-switch.vue';
import MdIcon from '@/components/icons/md-icon.vue';
import { Button } from '@/components/ui/button';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import {
  CHAT_ADAPTER_FAILURE_MESSAGE_KEYS,
  CHAT_AGENT_ERROR_MESSAGE_KEYS,
  type ChatAdapterFailureReason,
  type ChatAgentErrorCode,
  type ChatPendingPermission,
} from '@/lib/models/chat';
import { ICON_NAMES } from '@/lib/models/ui';
import { LinkService } from '@/lib/services/link-service';
import { useChatStore } from '@/stores/chat-store';

import MdChatComposer from './components/md-chat-composer.vue';
import MdChatPermissionCard from './components/md-chat-permission-card.vue';
import MdChatToolCall from './components/md-chat-tool-call.vue';

const { t } = useI18n({ useScope: 'global' });
const chatStore = useChatStore();

const timelineElement = ref<HTMLElement | null>(null);
const composerElement = ref<InstanceType<typeof MdChatComposer> | null>(null);

// Provider probing is lazy: it only runs once the user actually opens the
// chat page, never during application startup.
onMounted(() => {
  if (!chatStore.probed && !chatStore.probing) void chatStore.probeAgents();
});

const showStartPanel = computed(
  () =>
    chatStore.probed &&
    chatStore.providers.length > 0 &&
    (chatStore.sessionPhase === 'idle' || chatStore.sessionPhase === 'starting')
);
const showTranscript = computed(() => chatStore.sessionPhase === 'active' || chatStore.sessionPhase === 'ended');

function resolveFailureMessage(code: string | null): string | null {
  if (!code) return null;
  const key =
    CHAT_AGENT_ERROR_MESSAGE_KEYS[code as ChatAgentErrorCode] ??
    CHAT_ADAPTER_FAILURE_MESSAGE_KEYS[code as ChatAdapterFailureReason];
  return key ? t(key) : t('chat.errors.unknown');
}

const startErrorMessage = computed(() => resolveFailureMessage(chatStore.startErrorCode));
const runErrorMessage = computed(() => resolveFailureMessage(chatStore.runError?.code ?? null));

function permissionToolName(permission: ChatPendingPermission): string | null {
  const entry = chatStore.entries.find(item => item.kind === 'tool' && item.id === permission.toolCallId);
  return entry?.kind === 'tool' ? entry.name : null;
}

async function openExternalLink(url: string) {
  try {
    await LinkService.open(url);
  } catch {
    // The opener failure is reported by the link service's own diagnostics.
  }
}

function sendPrompt(text: string) {
  void chatStore.sendPrompt(text);
}

function startSession() {
  void chatStore.startSession().then(() => {
    if (chatStore.sessionPhase === 'active') composerElement.value?.focus();
  });
}

/** Returns to the start panel so the next session can pick another provider. */
function newSession() {
  void chatStore.closeSession();
}

// Streaming content grows entry by entry and delta by delta; track a cheap
// revision so the watcher fires on both without deep-watching the transcript.
const transcriptRevision = computed(() =>
  chatStore.entries.reduce(
    (revision, entry) => revision + (entry.kind === 'message' ? entry.text.length : entry.status.length),
    chatStore.entries.length
  )
);

// Ask-AI handoffs seed the composer through the chat store. The composer only
// exists for an active or ended session, so a seed may wait behind the start
// panel; this watcher applies it as soon as the composer is rendered.
watch(
  [() => chatStore.composerSeed, composerElement],
  ([seed, composer]) => {
    if (!seed || !composer) return;
    composer.setDraft(seed.text);
    chatStore.consumeComposerSeed();
  },
  { flush: 'post' }
);

watch(transcriptRevision, async () => {
  await nextTick();
  const element = timelineElement.value;
  if (!element) return;
  // Streaming must not yank the viewport away from a user who scrolled up to
  // read earlier output; only pinned-to-bottom positions follow the stream.
  const distanceToBottom = element.scrollHeight - element.scrollTop - element.clientHeight;
  if (distanceToBottom < 120) element.scrollTop = element.scrollHeight;
});
</script>

<template>
  <MdPageShell class="@container/chat" content-mode="workspace" :title="t('navigation.chat')">
    <template #subtitle>
      <p class="mt-1.5 mb-0 text-sm leading-relaxed text-muted-foreground">
        <template v-if="chatStore.sessionPhase === 'active' || chatStore.sessionPhase === 'ended'">
          {{ chatStore.providerDisplayName }}
          <span v-if="chatStore.sessionMutationsEnabled" class="chat-mutations-badge">{{ t('chat.mutationsOn') }}</span>
        </template>
        <template v-else>{{ t('chat.subtitle') }}</template>
      </p>
    </template>
    <template v-if="chatStore.sessionPhase === 'active' || chatStore.sessionPhase === 'ended'" #actions>
      <Button type="button" variant="outline" size="sm" @click="newSession">
        <MdIcon :name="ICON_NAMES.refresh" :size="14" />
        {{ t('chat.newSession') }}
      </Button>
    </template>

    <div v-if="chatStore.probing && !chatStore.probed" class="chat-center-notice" role="status">
      <i class="md-operational-motion chat-spinner" aria-hidden="true" />
      <p>{{ t('chat.probing') }}</p>
    </div>

    <MdEmptyState
      v-else-if="chatStore.probed && chatStore.providers.length === 0"
      :icon-name="ICON_NAMES.chat"
      :title="t('chat.noAgentsTitle')"
      :description="t('chat.noAgentsDescription')"
    >
      <Button type="button" variant="outline" size="sm" @click="chatStore.probeAgents()">
        <MdIcon :name="ICON_NAMES.refresh" :size="14" />
        {{ t('chat.probeAgain') }}
      </Button>
    </MdEmptyState>

    <div v-else-if="showStartPanel" class="chat-start-panel">
      <section class="chat-start-card">
        <h2 class="chat-start-title">{{ t('chat.startTitle') }}</h2>
        <p class="chat-start-description">{{ t('chat.startDescription') }}</p>
        <label class="chat-start-field">
          <span>{{ t('chat.providerLabel') }}</span>
          <Select
            :model-value="chatStore.selectedProviderId ?? undefined"
            :disabled="chatStore.sessionPhase === 'starting'"
            @update:model-value="chatStore.selectProvider(String($event))"
          >
            <SelectTrigger class="chat-provider-select">
              <SelectValue :placeholder="t('chat.providerLabel')" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="provider in chatStore.providers" :key="provider.id" :value="provider.id">
                {{ provider.displayName }}
              </SelectItem>
            </SelectContent>
          </Select>
        </label>
        <div class="chat-start-field chat-mutations-field">
          <div class="chat-mutations-text">
            <span>{{ t('chat.mutationsLabel') }}</span>
            <small>{{ t('chat.mutationsHint') }}</small>
          </div>
          <MdSwitch
            :model-value="chatStore.mutationsEnabled"
            :disabled="chatStore.sessionPhase === 'starting'"
            @update:model-value="chatStore.setMutationsEnabled($event)"
          />
        </div>
        <p v-if="startErrorMessage" class="chat-start-error" role="alert">{{ startErrorMessage }}</p>
        <div class="chat-start-actions">
          <Button
            type="button"
            :disabled="!chatStore.selectedProviderId || chatStore.sessionPhase === 'starting'"
            @click="startSession"
          >
            <i
              v-if="chatStore.sessionPhase === 'starting'"
              class="md-operational-motion chat-spinner"
              aria-hidden="true"
            />
            {{ chatStore.sessionPhase === 'starting' ? t('chat.starting') : t('chat.startSession') }}
          </Button>
        </div>
      </section>
    </div>

    <template v-else-if="showTranscript">
      <div v-if="chatStore.sessionPhase === 'ended'" class="chat-ended-banner" role="status">
        <span>{{ t('chat.sessionEnded') }}</span>
      </div>
      <section v-if="chatStore.planEntries.length" class="chat-plan" :aria-label="t('chat.planTitle')">
        <h2>{{ t('chat.planTitle') }}</h2>
        <ul>
          <li v-for="(entry, index) in chatStore.planEntries" :key="index" :data-status="entry.status">
            <MdIcon
              :name="entry.status === 'completed' ? ICON_NAMES.check : ICON_NAMES.minus"
              :size="13"
              aria-hidden="true"
            />
            <span>{{ entry.content }}</span>
          </li>
        </ul>
      </section>
      <div ref="timelineElement" class="chat-timeline scrollbar-stable" aria-live="polite">
        <p v-if="chatStore.sessionActive && chatStore.entries.length === 0" class="chat-transcript-hint">
          {{ t('chat.transcriptEmptyHint') }}
        </p>
        <template v-for="entry in chatStore.entries" :key="entry.id">
          <div v-if="entry.kind === 'message'" class="chat-message" :class="[`is-${entry.role}`]">
            <div v-if="entry.thought" class="chat-message-thought">
              <span class="chat-thought-label">{{ t('chat.thinkingLabel') }}</span>
              <p>{{ entry.text }}</p>
            </div>
            <MdSafeRichText v-else :content="entry.text" @open-link="openExternalLink" />
            <span v-if="entry.streaming" class="chat-streaming-indicator md-operational-motion" aria-hidden="true" />
          </div>
          <MdChatToolCall v-else :entry="entry" />
        </template>
      </div>
      <MdChatPermissionCard
        v-for="permission in chatStore.pendingPermissions"
        :key="permission.requestId"
        :permission="permission"
        :tool-name="permissionToolName(permission)"
        @resolve="chatStore.resolvePermission"
      />
      <div v-if="chatStore.runError" class="chat-run-error" role="alert">
        <strong>{{ runErrorMessage }}</strong>
        <span v-if="chatStore.runError.message">{{ chatStore.runError.message }}</span>
      </div>
      <MdChatComposer
        ref="composerElement"
        :sending="chatStore.sending"
        :running="chatStore.runInFlight"
        :disabled="!chatStore.sessionActive"
        @send="sendPrompt"
        @stop="chatStore.cancelRun()"
      />
    </template>
  </MdPageShell>
</template>

<style scoped>
@reference "@assets/main.css";

.chat-center-notice {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  @apply text-muted-foreground;
}

.chat-center-notice p {
  margin: 0;
}

.chat-spinner {
  width: 14px;
  height: 14px;
  flex: none;
  border-width: 1.5px;
  border-style: solid;
  border-radius: 50%;
  border-color: var(--border);
  border-top-color: var(--primary);
  animation: chat-spin 0.8s linear infinite;
}

.chat-start-panel {
  display: flex;
  min-height: 0;
  flex: 1;
  align-items: center;
  justify-content: center;
}

.chat-start-card {
  display: flex;
  width: 100%;
  max-width: 460px;
  flex-direction: column;
  gap: 14px;
  border-width: 1px;
  border-radius: 12px;
  padding: 20px;
  @apply border-border bg-card;
}

.chat-start-title {
  margin: 0;
  font-size: 16px;
  font-weight: 600;
  @apply text-foreground;
}

.chat-start-description {
  margin: 0;
  line-height: 1.6;
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
}

.chat-start-field {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  font-size: var(--font-content-secondary);
  @apply text-foreground;
}

.chat-provider-select {
  width: 220px;
}

.chat-mutations-field {
  border-width: 1px;
  border-radius: 8px;
  padding: 8px 10px;
  @apply border-border;
}

.chat-mutations-text {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 2px;
}

.chat-mutations-text small {
  @apply text-muted-foreground;
  font-size: 11px;
  line-height: 1.45;
}

.chat-start-error {
  margin: 0;
  font-size: var(--font-content-secondary);
  @apply text-destructive;
}

.chat-start-actions {
  display: flex;
  justify-content: flex-end;
}

.chat-mutations-badge {
  margin-inline-start: 8px;
  border-width: 1px;
  border-radius: 999px;
  padding: 1px 8px;
  font-size: 11px;
  @apply border-primary/40 text-primary;
}

.chat-ended-banner {
  display: flex;
  flex: none;
  align-items: center;
  border-width: 1px;
  border-radius: 8px;
  padding: 8px 12px;
  font-size: var(--font-content-secondary);
  @apply border-border bg-muted/50 text-muted-foreground;
}

.chat-plan {
  flex: none;
  border-width: 1px;
  border-radius: 10px;
  padding: 8px 12px;
  @apply border-border bg-card;
}

.chat-plan h2 {
  margin: 0 0 4px;
  font-size: var(--font-content-secondary);
  font-weight: 600;
  @apply text-foreground;
}

.chat-plan ul {
  display: flex;
  margin: 0;
  flex-direction: column;
  gap: 3px;
  padding: 0;
  list-style: none;
}

.chat-plan li {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: var(--font-content-secondary);
  @apply text-muted-foreground;
}

.chat-plan li[data-status='completed'] {
  @apply text-foreground;
}

.chat-timeline {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  gap: 10px;
  overflow-y: auto;
  padding-inline-end: 6px;
}

.chat-transcript-hint {
  margin: auto;
  max-width: 420px;
  text-align: center;
  line-height: 1.6;
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
}

.chat-message {
  max-width: 85%;
  border-radius: 12px;
  padding: 9px 13px;
  font-size: var(--font-content-body);
  line-height: 1.6;
}

.chat-message.is-user {
  align-self: flex-end;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  @apply bg-primary text-primary-foreground;
}

.chat-message.is-agent {
  align-self: flex-start;
  @apply bg-card text-card-foreground;
  border-width: 1px;
  @apply border-border;
}

.chat-message-thought {
  font-size: var(--font-content-secondary);
  @apply text-muted-foreground;
}

.chat-message-thought p {
  margin: 4px 0 0;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.chat-thought-label {
  font-weight: 600;
  letter-spacing: 0.02em;
}

.chat-streaming-indicator {
  display: inline-block;
  width: 7px;
  height: 7px;
  margin-inline-start: 6px;
  border-radius: 50%;
  @apply bg-primary;
  animation: chat-stream-pulse 1.1s ease-in-out infinite;
}

.chat-run-error {
  display: flex;
  flex: none;
  flex-direction: column;
  gap: 2px;
  border-width: 1px;
  border-radius: 8px;
  padding: 8px 12px;
  font-size: var(--font-content-secondary);
  @apply border-destructive/40 bg-card text-destructive;
}

.chat-run-error span {
  @apply text-muted-foreground;
}

@keyframes chat-spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes chat-stream-pulse {
  0%,
  100% {
    opacity: 0.35;
  }
  50% {
    opacity: 1;
  }
}
</style>
