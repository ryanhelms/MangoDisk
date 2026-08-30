<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';

import MdDialogContent from '@/components/custom/md-dialog-content.vue';
import MdResultCheckbox from '@/components/custom/md-result-checkbox.vue';
import MdIcon from '@/components/icons/md-icon.vue';
import { Button } from '@/components/ui/button';
import { Dialog, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import {
  PROCESS_CLASSIFICATION_LABEL_KEYS,
  PROCESS_CLASSIFICATION_ORDER,
  PROCESS_END_ITEM_STATUS_LABEL_KEYS,
  PROCESS_END_REFUSAL_LABEL_KEYS,
  processEndDecisionRefusal,
  type ProcessClassification,
  type ProcessEndItemStatus,
  type ProcessEndMode,
  type ProcessEndPlan,
  type ProcessEndPlanItem,
  type ProcessEndResult,
} from '@/lib/models/process';
import { EMPTY_DISPLAY_TEXT, ICON_NAMES } from '@/lib/models/ui';
import { FormatUtils } from '@/lib/utils/format';

const props = withDefaults(
  defineProps<{
    plan: ProcessEndPlan | null;
    result: ProcessEndResult | null;
    mode: ProcessEndMode;
    executing?: boolean;
  }>(),
  { executing: false }
);

const emit = defineEmits<{
  'update:mode': [mode: ProcessEndMode];
  cancel: [];
  confirm: [confirmed: boolean];
  closeResult: [];
}>();

const { t } = useI18n({ useScope: 'global' });

const open = computed({
  get: () => props.plan !== null || props.result !== null,
  set: value => {
    if (value) return;
    if (props.executing) return;
    if (props.result) emit('closeResult');
    else emit('cancel');
  },
});

const confirmedChecked = ref(false);
watch(
  () => props.plan?.planId,
  () => {
    confirmedChecked.value = false;
  }
);

interface PlanGroup {
  classification: ProcessClassification;
  items: ProcessEndPlanItem[];
}

const planGroups = computed<PlanGroup[]>(() => {
  if (!props.plan) return [];
  const groups = new Map<ProcessClassification, ProcessEndPlanItem[]>();
  for (const item of props.plan.items) {
    const items = groups.get(item.classification) ?? [];
    items.push(item);
    groups.set(item.classification, items);
  }
  return PROCESS_CLASSIFICATION_ORDER.filter(classification => groups.has(classification)).map(classification => ({
    classification,
    items: groups.get(classification) ?? [],
  }));
});

const allowedCount = computed(() => props.plan?.items.filter(item => item.decision === 'allowed').length ?? 0);
const refusedCount = computed(() => (props.plan ? props.plan.items.length - allowedCount.value : 0));

function refusalLabel(item: ProcessEndPlanItem): string | null {
  const refusal = processEndDecisionRefusal(item.decision);
  return refusal ? t(PROCESS_END_REFUSAL_LABEL_KEYS[refusal]) : null;
}

const modeOptions: Array<{ mode: ProcessEndMode; labelKey: string; hintKey: string }> = [
  { mode: 'graceful', labelKey: 'processes.end.modes.graceful', hintKey: 'processes.end.modeHints.graceful' },
  { mode: 'force', labelKey: 'processes.end.modes.force', hintKey: 'processes.end.modeHints.force' },
];

function confirm() {
  if (!confirmedChecked.value || !allowedCount.value || props.executing) return;
  emit('confirm', confirmedChecked.value);
}

const SUCCESS_STATUSES: ReadonlySet<ProcessEndItemStatus> = new Set(['ended', 'endedAfterForce', 'alreadyExited']);

function resultIcon(status: ProcessEndItemStatus) {
  if (SUCCESS_STATUSES.has(status)) return ICON_NAMES.check;
  if (status === 'refused') return ICON_NAMES.minus;
  return ICON_NAMES.info;
}

function resultStatusWarning(status: ProcessEndItemStatus): boolean {
  return !SUCCESS_STATUSES.has(status) && status !== 'refused';
}

const remainingText = computed(() => {
  if (!props.result || !props.result.remainingPids.length) return '';
  return props.result.remainingPids.map(pid => String(pid)).join(', ');
});
</script>

<template>
  <Dialog v-model:open="open">
    <MdDialogContent
      class="flex max-h-[84vh] min-h-0 w-[calc(100%-3rem)] max-w-[560px] flex-col gap-0 overflow-hidden p-0"
    >
      <template v-if="plan && !result">
        <DialogHeader class="flex-none px-6 pt-6 pr-14">
          <DialogTitle>{{ t('processes.end.dialogTitle') }}</DialogTitle>
          <DialogDescription>{{ t('processes.end.dialogDescription') }}</DialogDescription>
        </DialogHeader>

        <div class="process-end-body scrollbar-stable">
          <section v-for="group in planGroups" :key="group.classification" class="process-end-group">
            <h3>{{ t(PROCESS_CLASSIFICATION_LABEL_KEYS[group.classification]) }}</h3>
            <div v-for="item in group.items" :key="item.pid" class="process-end-item">
              <span class="process-end-item-name" :title="item.name">{{ item.name || EMPTY_DISPLAY_TEXT }}</span>
              <span class="process-end-item-pid">{{ item.pid }}</span>
              <span v-if="refusalLabel(item)" class="process-end-item-refusal">{{ refusalLabel(item) }}</span>
              <MdIcon v-else :name="ICON_NAMES.check" :size="14" class="process-end-item-allowed" />
            </div>
          </section>
          <p v-if="refusedCount" class="process-end-refused-note">
            {{ t('processes.end.refusedNote', { count: FormatUtils.integer(refusedCount) }, refusedCount) }}
          </p>

          <fieldset class="process-end-modes">
            <legend>{{ t('processes.end.modeLabel') }}</legend>
            <button
              v-for="option in modeOptions"
              :key="option.mode"
              type="button"
              role="radio"
              :aria-checked="mode === option.mode"
              class="process-end-mode"
              :class="{ selected: mode === option.mode }"
              :disabled="executing"
              @click="emit('update:mode', option.mode)"
            >
              <strong>{{ t(option.labelKey) }}</strong>
              <small>{{ t(option.hintKey) }}</small>
            </button>
          </fieldset>

          <label class="process-end-confirm" :class="{ disabled: executing }">
            <MdResultCheckbox
              :checked="confirmedChecked"
              :disabled="executing"
              @update:checked="confirmedChecked = $event"
            />
            <span>{{
              mode === 'force' ? t('processes.end.confirmCheckboxForce') : t('processes.end.confirmCheckbox')
            }}</span>
          </label>
        </div>

        <DialogFooter class="flex-none border-t border-border/70 px-6 py-3.5">
          <Button variant="outline" type="button" :disabled="executing" @click="emit('cancel')">
            {{ t('common.cancel') }}
          </Button>
          <Button
            variant="destructive"
            type="button"
            :disabled="!confirmedChecked || !allowedCount || executing"
            @click="confirm"
          >
            <i v-if="executing" class="md-operational-motion process-end-spinner" aria-hidden="true" />
            {{
              executing
                ? t('processes.end.executing')
                : t('processes.end.confirmAction', { count: FormatUtils.integer(allowedCount) }, allowedCount)
            }}
          </Button>
        </DialogFooter>
      </template>

      <template v-else-if="result">
        <DialogHeader class="flex-none px-6 pt-6 pr-14">
          <DialogTitle>{{ t('processes.end.resultTitle') }}</DialogTitle>
          <DialogDescription>{{
            t(
              'processes.end.resultSummary',
              {
                ended: FormatUtils.integer(result.endedCount),
                requested: FormatUtils.integer(result.requestedCount),
              },
              result.requestedCount
            )
          }}</DialogDescription>
        </DialogHeader>

        <div class="process-end-body scrollbar-stable">
          <p v-if="result.remainingPids.length" class="process-end-remaining" role="status">
            <MdIcon :name="ICON_NAMES.info" :size="15" />
            <span>{{
              t(
                'processes.end.resultRemaining',
                { count: FormatUtils.integer(result.remainingPids.length), pids: remainingText },
                result.remainingPids.length
              )
            }}</span>
          </p>
          <section class="process-end-group">
            <div v-for="item in result.items" :key="item.pid" class="process-end-item">
              <span class="process-end-item-status" :class="{ warning: resultStatusWarning(item.status) }">
                <MdIcon :name="resultIcon(item.status)" :size="12" />
              </span>
              <span class="process-end-item-name" :title="item.name">{{ item.name || EMPTY_DISPLAY_TEXT }}</span>
              <span class="process-end-item-pid">{{ item.pid }}</span>
              <span class="process-end-item-result-label">
                {{ t(PROCESS_END_ITEM_STATUS_LABEL_KEYS[item.status]) }}
                <small v-if="item.refusal">{{ t(PROCESS_END_REFUSAL_LABEL_KEYS[item.refusal]) }}</small>
              </span>
            </div>
          </section>
        </div>

        <DialogFooter class="flex-none border-t border-border/70 px-6 py-3.5">
          <Button variant="outline" type="button" @click="emit('closeResult')">{{ t('common.close') }}</Button>
        </DialogFooter>
      </template>
    </MdDialogContent>
  </Dialog>
</template>

<style scoped>
@reference "@assets/main.css";

.process-end-body {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  gap: 14px;
  overflow-y: auto;
  padding: 14px 24px;
}

.process-end-group {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.process-end-group h3 {
  margin: 0 0 2px;
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
  font-weight: 600;
}

.process-end-item {
  display: grid;
  min-height: 28px;
  grid-template-columns: minmax(0, 1fr) 64px max-content;
  align-items: center;
  gap: 10px;
  font-size: var(--font-content-body);
}

.process-end-item-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.process-end-item-pid {
  @apply text-muted-foreground;
  text-align: right;
  font-variant-numeric: tabular-nums;
}

.process-end-item-refusal {
  @apply text-warning-foreground;
  font-size: var(--font-content-secondary);
}

.process-end-item-allowed {
  justify-self: end;
  @apply text-success;
}

.process-end-item-status {
  display: grid;
  width: 20px;
  height: 20px;
  place-items: center;
  border-radius: 50%;
  @apply text-success;
  background: var(--surface-success-subtle);
}

.process-end-item-status.warning {
  @apply text-warning-foreground;
  background: var(--surface-warning-subtle);
}

.process-end-item:has(.process-end-item-status) {
  grid-template-columns: 24px minmax(0, 1fr) 64px max-content;
}

.process-end-item-result-label {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  font-size: var(--font-content-secondary);
}

.process-end-item-result-label small {
  @apply text-muted-foreground;
}

.process-end-refused-note {
  margin: 0;
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
}

.process-end-modes {
  display: grid;
  margin: 0;
  border: 0;
  padding: 0;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
}

.process-end-modes legend {
  margin-bottom: 6px;
  padding: 0;
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
  font-weight: 600;
}

.process-end-mode {
  display: flex;
  flex-direction: column;
  gap: 3px;
  border-width: 1px;
  border-radius: 10px;
  padding: 9px 11px;
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
  @apply border-border;
}

.process-end-mode.selected {
  @apply border-primary;
  background: var(--surface-primary-subtle);
}

.process-end-mode:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}

.process-end-mode small {
  @apply text-muted-foreground;
  font-size: 11px;
  line-height: 1.45;
}

.process-end-confirm {
  display: flex;
  align-items: flex-start;
  gap: 9px;
  cursor: pointer;
  font-size: var(--font-content-body);
  line-height: 1.5;
}

.process-end-confirm.disabled {
  cursor: not-allowed;
  opacity: 0.6;
}

.process-end-confirm span {
  padding-top: 1px;
}

.process-end-spinner {
  width: 13px;
  height: 13px;
  border-width: 2px;
  border-style: solid;
  border-color: currentColor;
  border-right-color: transparent;
  border-radius: 50%;
  animation: process-end-spin 0.8s linear infinite;
}

.process-end-remaining {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  margin: 0;
  border-width: 1px;
  border-radius: 10px;
  padding: 9px 12px;
  @apply border-warning/40 text-warning-foreground;
  background: var(--surface-warning-subtle);
  font-size: var(--font-content-secondary);
  line-height: 1.5;
}

@keyframes process-end-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
