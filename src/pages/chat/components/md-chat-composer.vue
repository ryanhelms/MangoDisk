<script setup lang="ts">
import { nextTick, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';

import MdIcon from '@/components/icons/md-icon.vue';
import { Button } from '@/components/ui/button';
import { ICON_NAMES } from '@/lib/models/ui';

const props = withDefaults(
  defineProps<{
    /** True while a prompt invoke is in flight. */
    sending?: boolean;
    /** True while the agent runs a turn: the primary action becomes Stop. */
    running?: boolean;
    /** True when the session cannot accept prompts (closed or ended). */
    disabled?: boolean;
  }>(),
  {
    sending: false,
    running: false,
    disabled: false,
  }
);

const emit = defineEmits<{
  send: [text: string];
  stop: [];
}>();

const { t } = useI18n({ useScope: 'global' });
const draft = ref('');
const inputElement = ref<HTMLTextAreaElement | null>(null);

watch(
  () => props.running,
  running => {
    // After a run settles, the user usually follows up; restoring focus keeps
    // the keyboard flow uninterrupted without stealing it mid-run.
    if (!running) void focusInput();
  }
);

async function focusInput() {
  await nextTick();
  inputElement.value?.focus();
}

function submit() {
  if (props.disabled || props.sending || props.running) return;
  const text = draft.value.trim();
  if (!text) return;
  emit('send', text);
  draft.value = '';
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    submit();
  }
}

defineExpose({ focus: focusInput });
</script>

<template>
  <div class="md-chat-composer" :class="{ 'is-disabled': disabled }">
    <textarea
      ref="inputElement"
      v-model="draft"
      class="md-chat-composer-input scrollbar-stable"
      :placeholder="disabled ? t('chat.inputUnavailablePlaceholder') : t('chat.inputPlaceholder')"
      :disabled="disabled"
      :aria-label="t('chat.inputLabel')"
      rows="2"
      @keydown="onKeydown"
    />
    <div class="md-chat-composer-actions">
      <Button
        v-if="running"
        type="button"
        variant="outline"
        size="sm"
        :aria-label="t('chat.stop')"
        @click="emit('stop')"
      >
        <MdIcon :name="ICON_NAMES.close" :size="14" />
        {{ t('chat.stop') }}
      </Button>
      <Button
        v-else
        type="button"
        size="sm"
        :disabled="disabled || sending || !draft.trim()"
        :aria-label="t('chat.send')"
        @click="submit"
      >
        {{ t('chat.send') }}
      </Button>
    </div>
  </div>
</template>

<style scoped>
@reference "@assets/main.css";

.md-chat-composer {
  display: flex;
  flex: none;
  align-items: flex-end;
  gap: 10px;
  border-width: 1px;
  border-radius: 10px;
  padding: 8px 8px 8px 12px;
  @apply border-border bg-card;
}

.md-chat-composer:focus-within {
  @apply border-ring;
}

.md-chat-composer.is-disabled {
  opacity: 0.6;
}

.md-chat-composer-input {
  min-width: 0;
  flex: 1;
  resize: none;
  background: transparent;
  line-height: 1.5;
  outline: none;
  @apply text-foreground;
}

.md-chat-composer-input::placeholder {
  @apply text-muted-foreground;
}

.md-chat-composer-actions {
  display: flex;
  flex: none;
  align-items: center;
}
</style>
