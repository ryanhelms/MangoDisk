<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import { PROCESS_CLASSIFICATION_LABEL_KEYS, type ProcessClassification } from '@/lib/models/process';

const props = defineProps<{ classification: ProcessClassification }>();

const { t } = useI18n({ useScope: 'global' });
const label = computed(() => t(PROCESS_CLASSIFICATION_LABEL_KEYS[props.classification]));
</script>

<template>
  <span class="process-classification-badge" :data-classification="classification">{{ label }}</span>
</template>

<style scoped>
@reference "@assets/main.css";

.process-classification-badge {
  display: inline-flex;
  max-width: 100%;
  flex: none;
  align-items: center;
  overflow: hidden;
  border-width: 1px;
  border-radius: 999px;
  padding: 1px 8px;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 10.5px;
  line-height: 1.6;
  @apply border-border text-muted-foreground;
}

.process-classification-badge[data-classification='criticalSystem'] {
  @apply border-destructive/40 text-destructive;
  background: var(--surface-destructive-subtle);
}

.process-classification-badge[data-classification='systemService'] {
  @apply border-warning/40 text-warning-foreground;
  background: var(--surface-warning-subtle);
}

.process-classification-badge[data-classification='userApplication'] {
  @apply border-primary/40 text-primary;
  background: var(--surface-primary-subtle);
}

.process-classification-badge[data-classification='userBackground'] {
  background: var(--muted);
}
</style>
