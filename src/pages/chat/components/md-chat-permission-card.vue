<script setup lang="ts">
import { useI18n } from 'vue-i18n';

import MdIcon from '@/components/icons/md-icon.vue';
import { Button } from '@/components/ui/button';
import type { ChatPendingPermission, ChatPermissionOption } from '@/lib/models/chat';
import { ICON_NAMES } from '@/lib/models/ui';

const props = defineProps<{
  permission: ChatPendingPermission;
  /** Display name of the tool call awaiting authorization, when known. */
  toolName: string | null;
}>();

const emit = defineEmits<{
  resolve: [requestId: number, optionId: string | null];
}>();

const { t } = useI18n({ useScope: 'global' });

function buttonVariant(option: ChatPermissionOption): 'default' | 'outline' | 'destructive' {
  // Option kinds come from the agent; the variant only communicates whether
  // the choice authorizes or rejects the pending action.
  if (option.kind === 'allow_once' || option.kind === 'allow_always') return 'default';
  if (option.kind === 'reject_once' || option.kind === 'reject_always') return 'outline';
  return 'outline';
}

function resolve(optionId: string | null) {
  emit('resolve', props.permission.requestId, optionId);
}
</script>

<template>
  <section class="md-chat-permission" role="alertdialog" :aria-label="t('chat.permissionTitle')">
    <header class="md-chat-permission-header">
      <MdIcon :name="ICON_NAMES.shield" :size="16" />
      <strong>{{ permission.title ?? t('chat.permissionTitleFallback') }}</strong>
    </header>
    <p v-if="toolName" class="md-chat-permission-tool">
      {{ t('chat.permissionToolContext', { tool: toolName }) }}
    </p>
    <div class="md-chat-permission-actions">
      <Button
        v-for="option in permission.options"
        :key="option.option_id"
        type="button"
        size="sm"
        :variant="buttonVariant(option)"
        @click="resolve(option.option_id)"
      >
        {{ option.label }}
      </Button>
      <Button type="button" size="sm" variant="ghost" @click="resolve(null)">
        {{ t('chat.permissionDecline') }}
      </Button>
    </div>
  </section>
</template>

<style scoped>
@reference "@assets/main.css";

.md-chat-permission {
  display: flex;
  flex-direction: column;
  gap: 8px;
  border-width: 1px;
  border-radius: 10px;
  padding: 10px 12px;
  @apply border-primary/40 bg-card;
}

.md-chat-permission-header {
  display: flex;
  align-items: center;
  gap: 8px;
  @apply text-foreground;
}

.md-chat-permission-tool {
  margin: 0;
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
}

.md-chat-permission-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
</style>
