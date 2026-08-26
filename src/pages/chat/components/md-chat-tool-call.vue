<script setup lang="ts">
import { computed, ref } from 'vue';
import { useI18n } from 'vue-i18n';

import MdIcon from '@/components/icons/md-icon.vue';
import { CHAT_TOOL_KIND_ICONS, CHAT_TOOL_KIND_LABEL_KEYS, CHAT_TOOL_STATUS_LABEL_KEYS } from '@/lib/models/chat';
import type { ChatToolCallEntry } from '@/lib/models/chat';
import { ICON_NAMES } from '@/lib/models/ui';

const props = defineProps<{
  entry: ChatToolCallEntry;
}>();

const { t } = useI18n({ useScope: 'global' });
const argsExpanded = ref(false);

const kindLabel = computed(() => t(CHAT_TOOL_KIND_LABEL_KEYS[props.entry.toolKind]));
const statusLabel = computed(() => t(CHAT_TOOL_STATUS_LABEL_KEYS[props.entry.status]));
const running = computed(() => props.entry.status === 'pending' || props.entry.status === 'in_progress');
</script>

<template>
  <section class="md-chat-tool-call" :class="{ 'is-running': running }">
    <header class="md-chat-tool-call-header">
      <span class="md-chat-tool-call-icon" aria-hidden="true">
        <MdIcon :name="CHAT_TOOL_KIND_ICONS[entry.toolKind]" :size="15" />
      </span>
      <span class="md-chat-tool-call-name" :title="entry.name">{{ entry.name }}</span>
      <span class="md-chat-tool-call-kind">{{ kindLabel }}</span>
      <span class="md-chat-tool-call-status" :data-status="entry.status">
        <i v-if="running" class="md-operational-motion md-chat-tool-call-spinner" aria-hidden="true" />
        {{ statusLabel }}
      </span>
      <button
        v-if="entry.argsJson"
        type="button"
        class="md-chat-tool-call-args-toggle"
        :aria-expanded="argsExpanded"
        :aria-label="argsExpanded ? t('chat.toolArgsHide') : t('chat.toolArgsShow')"
        @click="argsExpanded = !argsExpanded"
      >
        <MdIcon :name="argsExpanded ? ICON_NAMES.chevronUp : ICON_NAMES.chevronDown" :size="14" />
      </button>
    </header>
    <pre v-if="argsExpanded && entry.argsJson" class="md-chat-tool-call-args scrollbar-stable">{{
      entry.argsJson
    }}</pre>
  </section>
</template>

<style scoped>
@reference "@assets/main.css";

.md-chat-tool-call {
  border-width: 1px;
  border-radius: 9px;
  padding: 6px 10px;
  @apply border-border bg-muted/40;
}

.md-chat-tool-call-header {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 8px;
  font-size: var(--font-content-secondary);
}

.md-chat-tool-call-icon {
  display: grid;
  flex: none;
  place-items: center;
  @apply text-muted-foreground;
}

.md-chat-tool-call-name {
  overflow: hidden;
  flex: 1;
  text-overflow: ellipsis;
  white-space: nowrap;
  @apply text-foreground;
}

.md-chat-tool-call-kind {
  flex: none;
  @apply text-muted-foreground;
}

.md-chat-tool-call-status {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 5px;
  @apply text-muted-foreground;
}

.md-chat-tool-call-status[data-status='completed'] {
  @apply text-primary;
}

.md-chat-tool-call-status[data-status='failed'] {
  @apply text-destructive;
}

.md-chat-tool-call-spinner {
  width: 10px;
  height: 10px;
  border-width: 1.5px;
  border-style: solid;
  border-radius: 50%;
  border-color: var(--border);
  border-top-color: var(--primary);
  animation: chat-tool-spin 0.8s linear infinite;
}

.md-chat-tool-call-args-toggle {
  display: grid;
  flex: none;
  place-items: center;
  border-radius: 5px;
  padding: 2px;
  cursor: pointer;
  @apply text-muted-foreground;
}

.md-chat-tool-call-args-toggle:hover {
  @apply bg-accent text-accent-foreground;
}

.md-chat-tool-call-args {
  max-height: 180px;
  margin: 6px 0 0;
  overflow: auto;
  border-radius: 6px;
  padding: 8px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 11px;
  line-height: 1.5;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  @apply bg-background text-muted-foreground;
}

@keyframes chat-tool-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
