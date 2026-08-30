<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';

import MdIcon from '@/components/icons/md-icon.vue';
import { Button } from '@/components/ui/button';
import {
  PROCESS_CLASSIFICATION_DESCRIPTION_KEYS,
  PROCESS_METRIC_ABSENCE_LABEL_KEYS,
  PROCESS_STATE_LABEL_KEYS,
  type ProcessMetricAbsence,
} from '@/lib/models/process';
import { EMPTY_DISPLAY_TEXT, ICON_NAMES } from '@/lib/models/ui';
import { ByteSizeService } from '@/lib/services/byte-size-service';
import { FormatUtils } from '@/lib/utils/format';
import { PathUtils } from '@/lib/utils/path';
import type { ProcessRow } from '@/stores/processes-store';

import MdProcessClassificationBadge from './md-process-classification-badge.vue';

const props = defineProps<{ row: ProcessRow }>();
const emit = defineEmits<{
  close: [];
  askAi: [key: string];
  endProcess: [key: string];
}>();

const { locale, t } = useI18n({ useScope: 'global' });
const closeButton = ref<InstanceType<typeof Button> | null>(null);

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') emit('close');
}

onMounted(() => {
  window.addEventListener('keydown', onKeydown);
  const element = (closeButton.value as unknown as { $el?: HTMLElement } | null)?.$el;
  element?.focus();
});

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown);
});

const sample = computed(() => props.row.sample);

const executableText = computed(() => {
  if (sample.value.executablePath !== null) return PathUtils.display(sample.value.executablePath);
  return t(PROCESS_METRIC_ABSENCE_LABEL_KEYS[sample.value.executablePathAbsence ?? 'notAvailable']);
});
const executableIsPath = computed(() => sample.value.executablePath !== null);

const ownerText = computed(() => {
  const name = sample.value.ownerName;
  const uid = sample.value.ownerUid;
  if (name && uid !== null) return `${name} (${uid})`;
  if (name) return name;
  if (uid !== null) return String(uid);
  return EMPTY_DISPLAY_TEXT;
});

const startedAtText = computed(() =>
  sample.value.startedAtMs > 0 ? FormatUtils.dateTime(sample.value.startedAtMs, locale.value) : EMPTY_DISPLAY_TEXT
);

const stateText = computed(() => t(PROCESS_STATE_LABEL_KEYS[sample.value.state]));
const classificationDescription = computed(() => t(PROCESS_CLASSIFICATION_DESCRIPTION_KEYS[props.row.classification]));

function formatRate(value: number | null, absence: ProcessMetricAbsence | null): string {
  if (value !== null) return `${ByteSizeService.bytes(value)}/s`;
  return absence ? t(PROCESS_METRIC_ABSENCE_LABEL_KEYS[absence]) : t('processes.notMeasuredYet');
}

function formatCpu(value: number | null): string {
  return value === null ? t('processes.notMeasuredYet') : `${value.toFixed(1)}%`;
}

function formatCount(value: number | null, absence: ProcessMetricAbsence | null): string {
  if (value !== null) return String(value);
  return absence ? t(PROCESS_METRIC_ABSENCE_LABEL_KEYS[absence]) : EMPTY_DISPLAY_TEXT;
}

const cpuText = computed(() => formatCpu(sample.value.cpuPercent));
const readText = computed(() => formatRate(sample.value.readBps, sample.value.ioAbsence));
const writeText = computed(() => formatRate(sample.value.writeBps, sample.value.ioAbsence));
const openFilesText = computed(() => formatCount(sample.value.openFileCount, sample.value.openFilesAbsence));
</script>

<template>
  <div class="process-drawer-backdrop" aria-hidden="true" @click="emit('close')" />
  <aside class="process-drawer" :aria-label="t('processes.details.title')" role="complementary">
    <header class="process-drawer-header">
      <div class="process-drawer-heading">
        <h2 :title="sample.name">{{ sample.name }}</h2>
        <MdProcessClassificationBadge :classification="row.classification" />
      </div>
      <Button ref="closeButton" variant="ghost" size="sm" :aria-label="t('common.close')" @click="emit('close')">
        <MdIcon :name="ICON_NAMES.close" :size="15" />
      </Button>
    </header>

    <div class="process-drawer-body scrollbar-stable">
      <section class="process-drawer-section">
        <dl>
          <div>
            <dt>{{ t('processes.columns.pid') }}</dt>
            <dd>{{ sample.pid }}</dd>
          </div>
          <div>
            <dt>{{ t('processes.details.parentPid') }}</dt>
            <dd>{{ sample.ppid }}</dd>
          </div>
          <div>
            <dt>{{ t('processes.details.owner') }}</dt>
            <dd>
              {{ ownerText }}
              <span v-if="sample.ownedByCurrentUser === true" class="process-drawer-owner-badge">{{
                t('processes.details.currentUserBadge')
              }}</span>
            </dd>
          </div>
          <div>
            <dt>{{ t('processes.details.state') }}</dt>
            <dd>{{ stateText }}</dd>
          </div>
          <div>
            <dt>{{ t('processes.details.threads') }}</dt>
            <dd>{{ sample.threadCount }}</dd>
          </div>
          <div>
            <dt>{{ t('processes.details.startedAt') }}</dt>
            <dd>{{ startedAtText }}</dd>
          </div>
        </dl>
      </section>

      <section class="process-drawer-section">
        <h3>{{ t('processes.details.executable') }}</h3>
        <p class="process-drawer-path" :class="{ missing: !executableIsPath }" :title="executableText">
          {{ executableText }}
        </p>
        <template v-if="row.applicationName">
          <h3>{{ t('processes.columns.application') }}</h3>
          <p>{{ row.applicationName }}</p>
        </template>
      </section>

      <section class="process-drawer-section">
        <h3>{{ t('processes.columns.classification') }}</h3>
        <p class="process-drawer-classification">{{ classificationDescription }}</p>
      </section>

      <section class="process-drawer-section">
        <h3>{{ t('processes.details.liveMetrics') }}</h3>
        <dl>
          <div>
            <dt>{{ t('processes.columns.cpu') }}</dt>
            <dd>{{ cpuText }}</dd>
          </div>
          <div>
            <dt>{{ t('processes.columns.rss') }}</dt>
            <dd>{{ ByteSizeService.bytes(sample.rssBytes) }}</dd>
          </div>
          <div>
            <dt>{{ t('processes.columns.readRate') }}</dt>
            <dd>{{ readText }}</dd>
          </div>
          <div>
            <dt>{{ t('processes.columns.writeRate') }}</dt>
            <dd>{{ writeText }}</dd>
          </div>
          <div>
            <dt>{{ t('processes.columns.openFiles') }}</dt>
            <dd>{{ openFilesText }}</dd>
          </div>
        </dl>
      </section>
    </div>

    <footer class="process-drawer-footer">
      <Button variant="outline" size="sm" @click="emit('askAi', row.key)">
        <MdIcon :name="ICON_NAMES.chat" :size="14" />
        {{ t('processes.askAiAboutProcess') }}
      </Button>
      <Button
        v-if="row.classification !== 'criticalSystem'"
        variant="destructive"
        size="sm"
        @click="emit('endProcess', row.key)"
      >
        {{ t('processes.endProcess') }}
      </Button>
    </footer>
  </aside>
</template>

<style scoped>
@reference "@assets/main.css";

.process-drawer-backdrop {
  position: absolute;
  z-index: 30;
  background: color-mix(in oklab, var(--foreground) 18%, transparent);
  inset: 0;
}

.process-drawer {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  z-index: 31;
  display: flex;
  width: min(360px, 92%);
  flex-direction: column;
  border-left-width: 1px;
  @apply border-border bg-card text-card-foreground;
  box-shadow: -12px 0 32px color-mix(in oklab, var(--foreground) 10%, transparent);
  animation: process-drawer-in 160ms ease-out;
}

@keyframes process-drawer-in {
  from {
    transform: translateX(24px);
    opacity: 0;
  }

  to {
    transform: translateX(0);
    opacity: 1;
  }
}

.process-drawer-header {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  border-bottom-width: 1px;
  padding: 12px 14px;
  @apply border-border;
}

.process-drawer-heading {
  display: flex;
  min-width: 0;
  flex-direction: column;
  align-items: flex-start;
  gap: 6px;
}

.process-drawer-heading h2 {
  overflow: hidden;
  margin: 0;
  max-width: 100%;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 15px;
  font-weight: 600;
}

.process-drawer-body {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  gap: 14px;
  overflow-y: auto;
  padding: 14px;
}

.process-drawer-section {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.process-drawer-section h3 {
  margin: 0;
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
  font-weight: 600;
}

.process-drawer-section p {
  margin: 0;
  overflow-wrap: anywhere;
  font-size: var(--font-content-body);
}

.process-drawer-section dl {
  display: flex;
  margin: 0;
  flex-direction: column;
  gap: 6px;
}

.process-drawer-section dl > div {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
}

.process-drawer-section dt {
  flex: none;
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
}

.process-drawer-section dd {
  margin: 0;
  text-align: right;
  font-size: var(--font-content-body);
  font-variant-numeric: tabular-nums;
}

.process-drawer-path {
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
}

.process-drawer-path.missing {
  font-style: italic;
}

.process-drawer-owner-badge {
  margin-inline-start: 6px;
  border-radius: 999px;
  padding: 1px 7px;
  @apply text-primary;
  background: var(--surface-primary-subtle);
  font-size: 10px;
}

.process-drawer-classification {
  line-height: 1.55;
}

.process-drawer-footer {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  border-top-width: 1px;
  padding: 10px 14px;
  @apply border-border;
}
</style>
