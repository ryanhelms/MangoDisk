<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import MdIconAction from '@/components/custom/md-icon-action.vue';
import MdResultCheckbox from '@/components/custom/md-result-checkbox.vue';
import MdResultTable from '@/components/custom/md-result-table.vue';
import MdIcon from '@/components/icons/md-icon.vue';
import { PROCESS_METRIC_ABSENCE_LABEL_KEYS, type ProcessMetricAbsence } from '@/lib/models/process';
import { EMPTY_DISPLAY_TEXT, ICON_NAMES } from '@/lib/models/ui';
import { ByteSizeService } from '@/lib/services/byte-size-service';
import type { ProcessRow } from '@/stores/processes-store';

import MdProcessClassificationBadge from './md-process-classification-badge.vue';
import type { ProcessSortDirection, ProcessSortKey } from '../process-view';

export interface ProcessTableEntry {
  key: string;
  row: ProcessRow;
  depth: number;
}

const props = defineProps<{
  entries: ProcessTableEntry[];
  sortKey: ProcessSortKey;
  sortDirection: ProcessSortDirection;
  /** Tree order replaces sorting; headers become passive labels. */
  treeView: boolean;
  selectedKeys: string[];
  /** Kill flow in flight: row actions stay visible but inert. */
  busy: boolean;
}>();

const emit = defineEmits<{
  toggleSort: [key: ProcessSortKey];
  toggleSelect: [key: string];
  setAllSelected: [selected: boolean];
  openDetails: [key: string];
  askAi: [key: string];
  endProcess: [key: string];
}>();

const { t } = useI18n({ useScope: 'global' });

const columns: Array<{ key: ProcessSortKey; labelKey: string; cellClass: string }> = [
  { key: 'name', labelKey: 'processes.columns.name', cellClass: 'cell-name' },
  { key: 'pid', labelKey: 'processes.columns.pid', cellClass: 'cell-pid' },
  { key: 'user', labelKey: 'processes.columns.user', cellClass: 'cell-user' },
  { key: 'cpu', labelKey: 'processes.columns.cpu', cellClass: 'cell-metric' },
  { key: 'rss', labelKey: 'processes.columns.rss', cellClass: 'cell-metric' },
  { key: 'readRate', labelKey: 'processes.columns.readRate', cellClass: 'cell-metric cell-read' },
  { key: 'writeRate', labelKey: 'processes.columns.writeRate', cellClass: 'cell-metric cell-write' },
  { key: 'openFiles', labelKey: 'processes.columns.openFiles', cellClass: 'cell-metric cell-open' },
  { key: 'application', labelKey: 'processes.columns.application', cellClass: 'cell-application' },
  { key: 'classification', labelKey: 'processes.columns.classification', cellClass: 'cell-classification' },
];

const selection = computed(() => new Set(props.selectedKeys));
const endableKeys = computed(() =>
  props.entries.filter(entry => entry.row.classification !== 'criticalSystem').map(entry => entry.key)
);
const allEndableSelected = computed(
  () => endableKeys.value.length > 0 && endableKeys.value.every(key => selection.value.has(key))
);
const someEndableSelected = computed(() => endableKeys.value.some(key => selection.value.has(key)));

function toggleAll(checked: boolean) {
  emit('setAllSelected', checked);
}

function sortIcon(key: ProcessSortKey) {
  if (props.treeView || key !== props.sortKey) return ICON_NAMES.arrowUpDown;
  return props.sortDirection === 'asc' ? ICON_NAMES.arrowUp : ICON_NAMES.arrowDown;
}

function onHeaderClick(key: ProcessSortKey) {
  if (!props.treeView) emit('toggleSort', key);
}

function formatCpu(value: number | null): string {
  return value === null ? EMPTY_DISPLAY_TEXT : `${value.toFixed(1)}%`;
}

function formatRate(value: number | null): string {
  return value === null ? EMPTY_DISPLAY_TEXT : `${ByteSizeService.bytes(value)}/s`;
}

function formatCount(value: number | null): string {
  return value === null ? EMPTY_DISPLAY_TEXT : String(value);
}

/** Explains a missing metric through its typed absence code, never a guess. */
function absenceTitle(value: number | null, absence: ProcessMetricAbsence | null): string | undefined {
  if (value !== null) return undefined;
  return absence ? t(PROCESS_METRIC_ABSENCE_LABEL_KEYS[absence]) : t('processes.notMeasuredYet');
}

function rowClass(key: string) {
  return { selected: selection.value.has(key) };
}
</script>

<template>
  <MdResultTable class="process-table">
    <template #header>
      <div class="process-grid process-header">
        <span class="cell-check">
          <MdResultCheckbox
            :checked="allEndableSelected"
            :indeterminate="!allEndableSelected && someEndableSelected"
            :disabled="!endableKeys.length"
            :aria-label="t('processes.selectAll')"
            @update:checked="toggleAll"
          />
        </span>
        <button
          v-for="column in columns"
          :key="column.key"
          type="button"
          class="process-header-cell"
          :class="[column.cellClass, { passive: treeView, active: !treeView && sortKey === column.key }]"
          :tabindex="treeView ? -1 : 0"
          @click="onHeaderClick(column.key)"
        >
          <span>{{ t(column.labelKey) }}</span>
          <MdIcon v-if="!treeView" :name="sortIcon(column.key)" :size="12" />
        </button>
        <span class="cell-actions" />
      </div>
    </template>

    <div
      v-for="entry in entries"
      :key="entry.key"
      class="process-grid process-row"
      :class="rowClass(entry.key)"
      role="button"
      tabindex="0"
      @click="emit('openDetails', entry.key)"
      @keydown.enter.prevent="emit('openDetails', entry.key)"
      @keydown.space.prevent="emit('openDetails', entry.key)"
    >
      <span class="cell-check" @click.stop @keydown.stop>
        <MdResultCheckbox
          :checked="selection.has(entry.key)"
          :disabled="entry.row.classification === 'criticalSystem' || busy"
          :aria-label="t('processes.selectRow', { name: entry.row.sample.name })"
          @update:checked="emit('toggleSelect', entry.key)"
        />
      </span>
      <span class="cell-name" :title="entry.row.sample.name">
        <span class="cell-name-indent" :style="{ paddingInlineStart: `${entry.depth * 16}px` }">
          {{ entry.row.sample.name }}
        </span>
      </span>
      <span class="cell-pid">{{ entry.row.sample.pid }}</span>
      <span class="cell-user" :title="entry.row.sample.ownerName ?? undefined">{{
        entry.row.sample.ownerName ?? EMPTY_DISPLAY_TEXT
      }}</span>
      <span class="cell-metric" :title="absenceTitle(entry.row.sample.cpuPercent, null)">{{
        formatCpu(entry.row.sample.cpuPercent)
      }}</span>
      <span class="cell-metric">{{ ByteSizeService.bytes(entry.row.sample.rssBytes) }}</span>
      <span class="cell-metric cell-read" :title="absenceTitle(entry.row.sample.readBps, entry.row.sample.ioAbsence)">{{
        formatRate(entry.row.sample.readBps)
      }}</span>
      <span
        class="cell-metric cell-write"
        :title="absenceTitle(entry.row.sample.writeBps, entry.row.sample.ioAbsence)"
        >{{ formatRate(entry.row.sample.writeBps) }}</span
      >
      <span
        class="cell-metric cell-open"
        :title="absenceTitle(entry.row.sample.openFileCount, entry.row.sample.openFilesAbsence)"
        >{{ formatCount(entry.row.sample.openFileCount) }}</span
      >
      <span class="cell-application" :title="entry.row.applicationName ?? undefined">{{
        entry.row.applicationName ?? EMPTY_DISPLAY_TEXT
      }}</span>
      <span class="cell-classification">
        <MdProcessClassificationBadge :classification="entry.row.classification" />
      </span>
      <span class="cell-actions" @click.stop @keydown.stop>
        <MdIconAction
          :label="t('processes.askAiAboutProcess')"
          appearance="result"
          variant="ghost"
          @click="emit('askAi', entry.key)"
        >
          <MdIcon :name="ICON_NAMES.chat" :size="14" />
        </MdIconAction>
        <MdIconAction
          v-if="entry.row.classification !== 'criticalSystem'"
          :label="t('processes.endProcess')"
          appearance="result"
          variant="ghost"
          destructive
          :disabled="busy"
          @click="emit('endProcess', entry.key)"
        >
          <MdIcon :name="ICON_NAMES.close" :size="14" />
        </MdIconAction>
      </span>
    </div>
  </MdResultTable>
</template>

<style scoped>
@reference "@assets/main.css";

.process-table {
  --result-table-content-inline-padding: 0px;

  min-height: 0;
  flex: 1;
  overflow: hidden;
  border-width: 1px;
  border-radius: 11px;
  @apply border-border/70 bg-card text-card-foreground;
}

.process-grid {
  display: grid;
  grid-template-columns:
    34px minmax(140px, 1.5fr) 60px minmax(72px, 0.8fr) 62px 86px 88px 88px 72px minmax(88px, 0.9fr)
    118px 76px;
  align-items: center;
  gap: 4px;
  width: 100%;
}

.process-header {
  min-height: 36px;
  padding: 4px 8px;
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
}

.process-header-cell {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 4px;
  border: 0;
  padding: 2px 4px;
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
}

.process-header-cell span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.process-header-cell.passive {
  cursor: default;
}

.process-header-cell.active {
  @apply text-foreground;
}

.process-header-cell:not(.passive):hover {
  @apply text-foreground;
}

.process-row {
  min-height: 38px;
  border-top-width: 1px;
  padding: 3px 8px;
  @apply border-border/60;
  cursor: pointer;
  font-size: var(--font-content-secondary);
}

.process-row:hover {
  @apply bg-muted/40;
}

.process-row.selected {
  background: var(--surface-primary-subtle);
}

.process-row:focus-visible {
  outline: 2px solid var(--ring);
  outline-offset: -2px;
}

.cell-check {
  display: flex;
  align-items: center;
  justify-content: center;
}

.cell-name {
  display: flex;
  min-width: 0;
  font-weight: 500;
  @apply text-foreground;
}

.cell-name-indent {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cell-pid {
  @apply text-muted-foreground;
  font-variant-numeric: tabular-nums;
}

.cell-user,
.cell-application {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cell-metric {
  text-align: right;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.cell-classification {
  display: flex;
  min-width: 0;
  justify-content: flex-start;
}

.cell-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 2px;
}

@container processes (max-width: 1180px) {
  .process-grid {
    grid-template-columns: 34px minmax(130px, 1.5fr) 60px minmax(64px, 0.8fr) 62px 86px 88px 88px 72px 118px 76px;
  }

  .cell-application {
    display: none;
  }
}

@container processes (max-width: 1020px) {
  .process-grid {
    grid-template-columns: 34px minmax(120px, 1.5fr) 60px minmax(56px, 0.8fr) 62px 86px 88px 118px 76px;
  }

  .cell-read,
  .cell-open {
    display: none;
  }
}

@container processes (max-width: 820px) {
  .process-grid {
    grid-template-columns: 34px minmax(110px, 1.5fr) 56px 62px 86px 118px 76px;
  }

  .cell-user,
  .cell-write {
    display: none;
  }
}
</style>
