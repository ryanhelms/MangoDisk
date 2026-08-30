<script setup lang="ts">
import { computed, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';

import MdEmptyState from '@/components/custom/md-empty-state.vue';
import MdPageShell from '@/components/custom/md-page-shell.vue';
import MdResultSearch from '@/components/custom/md-result-search.vue';
import MdSwitch from '@/components/custom/md-switch.vue';
import MdIcon from '@/components/icons/md-icon.vue';
import { Button } from '@/components/ui/button';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { ICON_NAMES } from '@/lib/models/ui';
import { FormatUtils } from '@/lib/utils/format';
import { useProcessesStore } from '@/stores/processes-store';

import MdProcessDetailsDrawer from './components/md-process-details-drawer.vue';
import MdProcessEndDialog from './components/md-process-end-dialog.vue';
import MdProcessTable, { type ProcessTableEntry } from './components/md-process-table.vue';
import {
  flattenProcessTree,
  nextProcessSort,
  sortProcessRows,
  type ProcessSortDirection,
  type ProcessSortKey,
} from './process-view';

const { locale, t } = useI18n({ useScope: 'global' });
const store = useProcessesStore();

// The page owns the live loop: it runs only while the page is active inside
// the shell's KeepAlive and the browser window reports visible focus (the
// store tracks the latter). Deactivation pauses scanning without discarding
// the last snapshot.
onMounted(() => store.startLiveUpdates());
onActivated(() => store.startLiveUpdates());
onDeactivated(() => store.stopLiveUpdates());
onBeforeUnmount(() => store.stopLiveUpdates());

const ALL_USERS_VALUE = '__all__';
const sortKey = ref<ProcessSortKey>('cpu');
const sortDirection = ref<ProcessSortDirection>('desc');
const treeView = ref(false);

const userFilterValue = computed(() => store.filterUser ?? ALL_USERS_VALUE);

const entries = computed<ProcessTableEntry[]>(() => {
  if (treeView.value && store.tree) {
    return flattenProcessTree(store.tree, store.rows, store.rowOrder).flatMap(position => {
      const row = store.rows[position.key];
      return row ? [{ key: position.key, row, depth: position.depth }] : [];
    });
  }
  const rows = store.rowOrder
    .map(key => store.rows[key])
    .filter((row): row is NonNullable<typeof row> => row !== undefined);
  return sortProcessRows(rows, sortKey.value, sortDirection.value, locale.value).map(row => ({
    key: row.key,
    row,
    depth: 0,
  }));
});

const initialLoading = computed(() => store.scanning && store.snapshotMeta === null);
const initialLoadFailed = computed(() => store.loadFailed && store.snapshotMeta === null);
const selectedCount = computed(() => store.selectedRows.length);
const killFlowBusy = computed(() => store.preparingEnd || store.executingEnd);

function toggleSort(key: ProcessSortKey) {
  const next = nextProcessSort(key, sortKey.value, sortDirection.value);
  sortKey.value = next.key;
  sortDirection.value = next.direction;
}

function setAllSelected(selected: boolean) {
  store.setRowsSelected(
    entries.value.map(entry => entry.key),
    selected
  );
}

function updateUserFilter(value: unknown) {
  const text = String(value);
  store.setUserFilter(text === ALL_USERS_VALUE ? null : text);
}

function endSingleProcess(key: string) {
  const row = store.rows[key];
  if (!row) return;
  void store.prepareEnd([row.sample.pid]);
}

function retryInitialScan() {
  void store.scanNow();
}
</script>

<template>
  <MdPageShell class="@container/processes" content-mode="workspace" :title="t('navigation.processes')">
    <template #subtitle>
      <p class="mt-1.5 mb-0 text-sm leading-relaxed text-muted-foreground">{{ t('processes.subtitle') }}</p>
    </template>
    <template #actions>
      <Button
        type="button"
        variant="outline"
        size="sm"
        :disabled="!store.snapshotMeta"
        @click="store.askAiAboutDiskActivity()"
      >
        <MdIcon :name="ICON_NAMES.chat" :size="14" />
        {{ t('processes.askAiDiskBusy') }}
      </Button>
      <Button
        type="button"
        variant="outline"
        size="sm"
        :disabled="!selectedCount || killFlowBusy"
        @click="store.prepareEnd()"
      >
        <MdIcon :name="ICON_NAMES.close" :size="14" />
        {{ t('processes.endSelected', { count: FormatUtils.integer(selectedCount) }, selectedCount) }}
      </Button>
    </template>

    <div class="process-page-body">
      <div class="process-toolbar">
        <MdResultSearch
          compact
          :model-value="store.filterName"
          :placeholder="t('processes.searchPlaceholder')"
          @update:model-value="store.setNameFilter($event)"
        />
        <Select :model-value="userFilterValue" @update:model-value="updateUserFilter">
          <SelectTrigger class="process-user-select" :aria-label="t('processes.userFilterLabel')">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem :value="ALL_USERS_VALUE">{{ t('processes.userFilterAll') }}</SelectItem>
            <SelectItem v-for="user in store.knownUsers" :key="user" :value="user">{{ user }}</SelectItem>
          </SelectContent>
        </Select>
        <label class="process-tree-toggle">
          <span>{{ t('processes.treeViewLabel') }}</span>
          <MdSwitch :model-value="treeView" @update:model-value="treeView = $event" />
        </label>
        <span class="process-toolbar-summary">
          <i v-if="store.refreshing" class="md-operational-motion process-refresh-dot" aria-hidden="true" />
          {{
            store.snapshotMeta
              ? t(
                  'processes.summaryCount',
                  { count: FormatUtils.integer(store.snapshotMeta.processCount) },
                  store.snapshotMeta.processCount
                )
              : ''
          }}
        </span>
      </div>

      <div v-if="initialLoading" class="process-center-notice" role="status">
        <i class="md-operational-motion process-spinner" aria-hidden="true" />
        <p>{{ t('processes.loadingTitle') }}</p>
      </div>

      <MdEmptyState
        v-else-if="initialLoadFailed"
        class="process-empty"
        :icon-name="ICON_NAMES.processes"
        :title="t('processes.loadFailedTitle')"
        :description="t('processes.loadFailedDescription')"
      >
        <Button type="button" variant="outline" size="sm" @click="retryInitialScan">
          <MdIcon :name="ICON_NAMES.refresh" :size="14" />
          {{ t('processes.retry') }}
        </Button>
      </MdEmptyState>

      <MdEmptyState
        v-else-if="!entries.length"
        class="process-empty"
        :icon-name="ICON_NAMES.search"
        :title="t('processes.emptyTitle')"
        :description="t('processes.emptyDescription')"
      />

      <MdProcessTable
        v-else
        :entries="entries"
        :sort-key="sortKey"
        :sort-direction="sortDirection"
        :tree-view="treeView"
        :selected-keys="store.selectedKeys"
        :busy="killFlowBusy"
        @toggle-sort="toggleSort"
        @toggle-select="store.toggleRowSelection"
        @set-all-selected="setAllSelected"
        @open-details="store.openDetails"
        @ask-ai="store.askAiAboutProcess"
        @end-process="endSingleProcess"
      />

      <MdProcessDetailsDrawer
        v-if="store.detailsRow"
        :row="store.detailsRow"
        @close="store.openDetails(null)"
        @ask-ai="store.askAiAboutProcess"
        @end-process="endSingleProcess"
      />
    </div>

    <MdProcessEndDialog
      :plan="store.endPlan"
      :result="store.endResult"
      :mode="store.endMode"
      :executing="store.executingEnd"
      @update:mode="store.setEndMode"
      @cancel="store.cancelEndPlan"
      @confirm="store.executeEnd"
      @close-result="store.dismissEndResult"
    />
  </MdPageShell>
</template>

<style scoped>
@reference "@assets/main.css";

.process-page-body {
  position: relative;
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  gap: 8px;
  overflow: hidden;
}

.process-toolbar {
  display: flex;
  flex: none;
  align-items: center;
  gap: 10px;
}

.process-user-select {
  width: 168px;
}

.process-tree-toggle {
  display: flex;
  flex: none;
  align-items: center;
  gap: 7px;
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
}

.process-toolbar-summary {
  display: flex;
  min-width: 0;
  flex: 1;
  align-items: center;
  justify-content: flex-end;
  gap: 7px;
  @apply text-muted-foreground;
  font-size: var(--font-content-secondary);
  white-space: nowrap;
}

.process-refresh-dot {
  width: 7px;
  height: 7px;
  flex: none;
  border-radius: 50%;
  @apply bg-primary;
  animation: process-refresh-pulse 1s ease-in-out infinite;
}

.process-center-notice {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  @apply text-muted-foreground;
}

.process-center-notice p {
  margin: 0;
}

.process-spinner {
  width: 14px;
  height: 14px;
  border-width: 1.5px;
  border-style: solid;
  border-radius: 50%;
  border-color: var(--border);
  border-top-color: var(--primary);
  animation: process-spin 0.8s linear infinite;
}

.process-empty {
  min-height: 0;
  flex: 1;
}

@keyframes process-spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes process-refresh-pulse {
  0%,
  100% {
    opacity: 0.35;
  }

  50% {
    opacity: 1;
  }
}
</style>
