import { defineStore } from 'pinia';

import { PAGE_IDS } from '@/lib/models/application-shell';
import {
  PROCESS_LIVE_REFRESH_INTERVAL_MS,
  PROCESS_SNAPSHOT_SCHEMA_VERSION,
  processRowKey,
  type ProcessAssociationInventoryStatus,
  type ProcessClassification,
  type ProcessEndMode,
  type ProcessEndPlan,
  type ProcessEndResult,
  type ProcessSample,
  type ProcessScanFilter,
  type ProcessScanView,
  type ProcessTree,
} from '@/lib/models/process';
import { LOG_DOMAINS, LOG_EVENTS } from '@/lib/models/telemetry';
import { ProcessService } from '@/lib/services/process-service';
import { LoggerService } from '@/lib/services/logger-service';
import {
  PROCESS_ASK_AI_TOP_WRITER_COUNT,
  buildDiskBusySeed,
  buildProcessAnalysisSeed,
} from '@/lib/utils/process-ask-ai';
import { normalizeError } from '@/lib/utils/error';

import { useAppStore } from './app-store';
import { useChatStore } from './chat-store';

/** Debounce between filter keystrokes and the next backend-filtered scan. */
const FILTER_DEBOUNCE_MS = 250;
/** Mirrors Core's MAX_FILTER_TEXT_BYTES so filter text never fails validation. */
const MAX_FILTER_TEXT_BYTES = 128;

/** One table row: the Core sample plus its Core-derived projections. */
export interface ProcessRow {
  /** Stable identity across refreshes: `${pid}:${startedAtMs}` (pid-reuse safe). */
  key: string;
  sample: ProcessSample;
  classification: ProcessClassification;
  applicationName: string | null;
}

export interface ProcessSnapshotMeta {
  snapshotId: string;
  capturedAtMs: number;
  sampleIntervalMs: number;
  logicalCpuCount: number;
  processCount: number;
  newProcessCount: number;
  exitedProcessCount: number;
}

interface ProcessesState {
  snapshotMeta: ProcessSnapshotMeta | null;
  tree: ProcessTree | null;
  rows: Record<string, ProcessRow>;
  /** Row keys in scanned (pid) order; the page derives sorted/tree order. */
  rowOrder: string[];
  associationsStatus: ProcessAssociationInventoryStatus | null;
  scanning: boolean;
  refreshing: boolean;
  scanInFlight: boolean;
  loadFailed: boolean;
  liveActive: boolean;
  browserActive: boolean;
  filterName: string;
  filterUser: string | null;
  /** Union of owner names ever seen, so the user filter keeps its options. */
  knownUsers: string[];
  selectedKeys: string[];
  detailsKey: string | null;
  endPlan: ProcessEndPlan | null;
  endMode: ProcessEndMode;
  preparingEnd: boolean;
  executingEnd: boolean;
  endResult: ProcessEndResult | null;
  liveTimer: ReturnType<typeof setTimeout> | null;
  filterTimer: ReturnType<typeof setTimeout> | null;
  browserActivityListener: (() => void) | null;
}

function clampFilterText(text: string): string {
  let result = text;
  while (result && new TextEncoder().encode(result).length > MAX_FILTER_TEXT_BYTES) {
    result = result.slice(0, -1);
  }
  return result;
}

/** Routes a finished seed through the chat store; never bypasses it. */
function handoffToChat(text: string) {
  useChatStore().seedComposer(text);
  useAppStore().navigate(PAGE_IDS.chat);
}

export const useProcessesStore = defineStore('processes', {
  state: (): ProcessesState => ({
    snapshotMeta: null,
    tree: null,
    rows: {},
    rowOrder: [],
    associationsStatus: null,
    scanning: false,
    refreshing: false,
    scanInFlight: false,
    loadFailed: false,
    liveActive: false,
    browserActive: true,
    filterName: '',
    filterUser: null,
    knownUsers: [],
    selectedKeys: [],
    detailsKey: null,
    endPlan: null,
    endMode: 'graceful',
    preparingEnd: false,
    executingEnd: false,
    endResult: null,
    liveTimer: null,
    filterTimer: null,
    browserActivityListener: null,
  }),
  getters: {
    selectedRows(state): ProcessRow[] {
      return state.selectedKeys
        .map(key => state.rows[key])
        .filter((row): row is ProcessRow => row !== undefined && row.classification !== 'criticalSystem');
    },
    selectedPids(): number[] {
      return this.selectedRows.map(row => row.sample.pid);
    },
    detailsRow(state): ProcessRow | null {
      return state.detailsKey ? (state.rows[state.detailsKey] ?? null) : null;
    },
  },
  actions: {
    currentScanFilter(): ProcessScanFilter {
      const nameContains = clampFilterText(this.filterName.trim());
      const user = clampFilterText((this.filterUser ?? '').trim());
      return { nameContains: nameContains || null, user: user || null };
    },
    /** One backend-filtered scan; refreshes rows in place on success. */
    async scanNow() {
      if (this.scanInFlight) return;
      this.scanInFlight = true;
      const firstLoad = this.snapshotMeta === null;
      if (firstLoad) this.scanning = true;
      else this.refreshing = true;
      try {
        const view = await ProcessService.scan(this.currentScanFilter());
        if (view.snapshot.schemaVersion !== PROCESS_SNAPSHOT_SCHEMA_VERSION) {
          throw new Error(`unsupported process snapshot schema version: ${view.snapshot.schemaVersion}`);
        }
        this.applyScanView(view);
        this.loadFailed = false;
      } catch (error) {
        // Background refresh failures keep the stale rows and stay out of the
        // global error toast; only the first load surfaces a visible failure.
        if (firstLoad) {
          this.loadFailed = true;
          useAppStore().reportError(error);
        } else {
          LoggerService.warn(LOG_DOMAINS.processes, LOG_EVENTS.processScanFailed, {
            diagnostic: normalizeError(error),
          });
        }
      } finally {
        this.scanInFlight = false;
        this.scanning = false;
        this.refreshing = false;
      }
    },
    /**
     * Applies one scan view keyed by pid+startedAtMs: existing rows are
     * mutated in place (no flicker), new rows appended, vanished rows and
     * their selections pruned.
     */
    applyScanView(view: ProcessScanView) {
      const classifications = new Map(view.classifications.map(entry => [entry.pid, entry.classification]));
      const applications = new Map(view.associations.matches.map(match => [match.pid, match.applicationName]));
      const seen = new Set<string>();
      const order: string[] = [];
      for (const sample of view.snapshot.processes) {
        const key = processRowKey(sample.pid, sample.startedAtMs);
        seen.add(key);
        order.push(key);
        // A missing classification entry cannot happen from the adapter;
        // criticalSystem fails closed so the row can never be kill-selected.
        const classification = classifications.get(sample.pid) ?? 'criticalSystem';
        const applicationName = applications.get(sample.pid) ?? null;
        const existing = this.rows[key];
        if (existing) {
          existing.sample = sample;
          existing.classification = classification;
          existing.applicationName = applicationName;
        } else {
          this.rows[key] = { key, sample, classification, applicationName };
        }
      }
      for (const key of Object.keys(this.rows)) {
        if (!seen.has(key)) delete this.rows[key];
      }
      this.rowOrder = order;
      this.selectedKeys = this.selectedKeys.filter(key => {
        const row = this.rows[key];
        return row !== undefined && row.classification !== 'criticalSystem';
      });
      if (this.detailsKey && !seen.has(this.detailsKey)) this.detailsKey = null;
      this.tree = view.tree;
      this.associationsStatus = view.associations.inventoryStatus;
      this.snapshotMeta = {
        snapshotId: view.snapshot.snapshotId,
        capturedAtMs: view.snapshot.capturedAtMs,
        sampleIntervalMs: view.snapshot.sampleIntervalMs,
        logicalCpuCount: view.snapshot.logicalCpuCount,
        processCount: view.snapshot.processes.length,
        newProcessCount: view.snapshot.newProcessCount,
        exitedProcessCount: view.snapshot.exitedProcessCount,
      };
      const knownUsers = new Set(this.knownUsers);
      for (const sample of view.snapshot.processes) {
        if (sample.ownerName) knownUsers.add(sample.ownerName);
      }
      this.knownUsers = [...knownUsers].sort((left, right) => left.localeCompare(right));
    },
    /** Starts the page-owned refresh loop; idempotent. */
    startLiveUpdates() {
      if (this.liveActive) return;
      this.liveActive = true;
      this.attachBrowserActivityListeners();
      void this.runLiveCycle();
    },
    stopLiveUpdates() {
      this.liveActive = false;
      this.clearLiveTimer();
      if (this.filterTimer) {
        clearTimeout(this.filterTimer);
        this.filterTimer = null;
      }
      this.browserActivityListener?.();
      this.browserActivityListener = null;
    },
    /** Self-scheduling refresh cycle; never overlaps the previous scan. */
    async runLiveCycle() {
      if (!this.liveActive || !this.browserActive) return;
      await this.scanNow();
      if (this.liveActive && this.browserActive && this.liveTimer === null) {
        this.liveTimer = setTimeout(() => {
          this.liveTimer = null;
          void this.runLiveCycle();
        }, PROCESS_LIVE_REFRESH_INTERVAL_MS);
      }
    },
    clearLiveTimer() {
      if (this.liveTimer) {
        clearTimeout(this.liveTimer);
        this.liveTimer = null;
      }
    },
    /** Pausing keeps the last snapshot; resuming scans immediately. */
    setBrowserActive(active: boolean) {
      if (this.browserActive === active) return;
      this.browserActive = active;
      if (!this.liveActive) return;
      if (active) {
        void this.runLiveCycle();
      } else {
        this.clearLiveTimer();
      }
    },
    attachBrowserActivityListeners() {
      if (this.browserActivityListener || typeof document === 'undefined' || typeof window === 'undefined') return;
      const sync = () => this.setBrowserActive(document.visibilityState === 'visible' && document.hasFocus());
      document.addEventListener('visibilitychange', sync);
      window.addEventListener('focus', sync);
      window.addEventListener('blur', sync);
      this.browserActivityListener = () => {
        document.removeEventListener('visibilitychange', sync);
        window.removeEventListener('focus', sync);
        window.removeEventListener('blur', sync);
      };
      sync();
    },
    setNameFilter(text: string) {
      this.filterName = text;
      this.scheduleFilterScan();
    },
    setUserFilter(user: string | null) {
      this.filterUser = user;
      this.scheduleFilterScan();
    },
    scheduleFilterScan() {
      if (this.filterTimer) clearTimeout(this.filterTimer);
      this.filterTimer = setTimeout(() => {
        this.filterTimer = null;
        if (!this.liveActive || !this.browserActive) return;
        this.clearLiveTimer();
        void this.runLiveCycle();
      }, FILTER_DEBOUNCE_MS);
    },
    toggleRowSelection(key: string) {
      const row = this.rows[key];
      if (!row || row.classification === 'criticalSystem') return;
      this.selectedKeys = this.selectedKeys.includes(key)
        ? this.selectedKeys.filter(item => item !== key)
        : [...this.selectedKeys, key];
    },
    setRowsSelected(keys: string[], selected: boolean) {
      const endable = new Set(
        keys.filter(key => this.rows[key] !== undefined && this.rows[key].classification !== 'criticalSystem')
      );
      this.selectedKeys = selected
        ? [...new Set([...this.selectedKeys, ...endable])]
        : this.selectedKeys.filter(key => !endable.has(key));
    },
    clearSelection() {
      this.selectedKeys = [];
    },
    openDetails(key: string | null) {
      this.detailsKey = key;
    },
    /** Prepares a kill plan for the given pids, or the current selection. */
    async prepareEnd(pids?: number[]) {
      const requested = pids ?? this.selectedPids;
      if (!requested.length || this.preparingEnd || this.executingEnd) return;
      this.preparingEnd = true;
      this.endResult = null;
      try {
        this.endPlan = await ProcessService.prepareEnd(requested);
        this.endMode = 'graceful';
      } catch (error) {
        LoggerService.warn(LOG_DOMAINS.processes, LOG_EVENTS.processEndPrepareFailed, {
          diagnostic: normalizeError(error),
        });
        useAppStore().reportError(error);
      } finally {
        this.preparingEnd = false;
      }
    },
    cancelEndPlan() {
      if (this.executingEnd) return;
      this.endPlan = null;
    },
    setEndMode(mode: ProcessEndMode) {
      this.endMode = mode;
    },
    /**
     * Executes the prepared plan. `confirmed` must come from the dialog's
     * explicit confirmation; Core turns anything else into a typed refusal.
     */
    async executeEnd(confirmed: boolean) {
      const plan = this.endPlan;
      if (!plan || this.executingEnd) return;
      this.executingEnd = true;
      try {
        const result = await ProcessService.executeEnd(plan, this.endMode, confirmed);
        this.endPlan = null;
        this.endResult = result;
        // remainingPids is the final authority: only those rows stay selected.
        const remaining = new Set(result.remainingPids);
        this.selectedKeys = this.selectedKeys.filter(key => {
          const row = this.rows[key];
          return row !== undefined && remaining.has(row.sample.pid);
        });
        // Execution changes the inventory; refresh immediately instead of
        // waiting for the next live tick.
        void this.scanNow();
      } catch (error) {
        // A stale, superseded, or expired plan can never execute; drop it so
        // the next attempt re-prepares against a fresh snapshot.
        this.endPlan = null;
        LoggerService.warn(LOG_DOMAINS.processes, LOG_EVENTS.processEndExecuteFailed, {
          diagnostic: normalizeError(error),
        });
        useAppStore().reportError(error);
      } finally {
        this.executingEnd = false;
      }
    },
    dismissEndResult() {
      this.endResult = null;
    },
    askAiAboutProcess(key: string) {
      const row = this.rows[key];
      if (!row) return;
      handoffToChat(
        buildProcessAnalysisSeed({
          sample: row.sample,
          classification: row.classification,
          applicationName: row.applicationName,
        })
      );
    },
    askAiAboutDiskActivity() {
      const meta = this.snapshotMeta;
      if (!meta) return;
      const writers = this.rowOrder
        .map(key => this.rows[key])
        .filter((row): row is ProcessRow => row !== undefined && (row.sample.writeBps ?? 0) > 0)
        .sort(
          (left, right) =>
            (right.sample.writeBps ?? 0) - (left.sample.writeBps ?? 0) || left.sample.pid - right.sample.pid
        )
        .slice(0, PROCESS_ASK_AI_TOP_WRITER_COUNT)
        .map(row => ({
          sample: row.sample,
          classification: row.classification,
          applicationName: row.applicationName,
        }));
      handoffToChat(buildDiskBusySeed({ capturedAtMs: meta.capturedAtMs, processCount: meta.processCount, writers }));
    },
  },
});
