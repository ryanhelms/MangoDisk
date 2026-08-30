import { createPinia, setActivePinia } from 'pinia';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type {
  ProcessClassification,
  ProcessEndPlan,
  ProcessEndPlanItem,
  ProcessEndResult,
  ProcessSample,
  ProcessScanView,
} from '@/lib/models/process';
import { LoggerService } from '@/lib/services/logger-service';
import { ProcessService } from '@/lib/services/process-service';

import { useAppStore } from './app-store';
import { useChatStore } from './chat-store';
import { useProcessesStore } from './processes-store';

function sample(overrides: Partial<ProcessSample> = {}): ProcessSample {
  return {
    pid: 100,
    ppid: 1,
    name: 'demo',
    executablePath: '/usr/bin/demo',
    executablePathAbsence: null,
    ownerUid: 1000,
    ownerName: 'ryan',
    ownedByCurrentUser: true,
    state: 'running',
    threadCount: 4,
    cpuUserTicks: 10,
    cpuKernelTicks: 5,
    cpuTicksPerSecond: 100,
    cpuPercent: 10,
    rssBytes: 4096,
    ioReadBytes: 0,
    ioWriteBytes: 0,
    ioAbsence: null,
    readBps: 0,
    writeBps: 0,
    openFileCount: 12,
    openFilesAbsence: null,
    startedAtMs: 111,
    ...overrides,
  };
}

function scanView(
  samples: ProcessSample[],
  overrides: {
    classifications?: Partial<Record<number, ProcessClassification>>;
    applications?: Partial<Record<number, string>>;
  } = {}
): ProcessScanView {
  return {
    snapshot: {
      schemaVersion: 1,
      snapshotId: 'scan-1',
      capturedAtMs: 1_000,
      sampleIntervalMs: 500,
      cpuTicksPerSecond: 100,
      logicalCpuCount: 8,
      newProcessCount: 0,
      exitedProcessCount: 0,
      processes: samples,
    },
    tree: { nodes: {}, roots: [] },
    associations: {
      inventoryStatus: 'available',
      matches: samples.map(item => ({
        pid: item.pid,
        applicationIdentifier: overrides.applications?.[item.pid] ? `app-${item.pid}` : null,
        applicationName: overrides.applications?.[item.pid] ?? null,
      })),
    },
    classifications: samples.map(item => ({
      pid: item.pid,
      classification: overrides.classifications?.[item.pid] ?? 'userBackground',
    })),
  };
}

function planItem(overrides: Partial<ProcessEndPlanItem> = {}): ProcessEndPlanItem {
  return {
    pid: 100,
    name: 'demo',
    startedAtMs: 111,
    classification: 'userBackground',
    decision: 'allowed',
    ...overrides,
  };
}

function plan(items: ProcessEndPlanItem[]): ProcessEndPlan {
  return { schemaVersion: 1, planId: 'plan-1', issuedAtMs: 1_000, items };
}

function endResult(overrides: Partial<ProcessEndResult> = {}): ProcessEndResult {
  return {
    planId: 'plan-1',
    mode: 'graceful',
    requestedCount: 1,
    endedCount: 1,
    failedCount: 0,
    remainingPids: [],
    items: [{ pid: 100, name: 'demo', status: 'ended', refusal: null }],
    elapsedMs: 120,
    ...overrides,
  };
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.restoreAllMocks();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('processes store snapshot ingestion', () => {
  it('builds rows, metadata, classifications, and associations from one scan', async () => {
    vi.spyOn(ProcessService, 'scan').mockResolvedValue(
      scanView([sample()], {
        classifications: { 100: 'userApplication' },
        applications: { 100: 'Demo App' },
      })
    );
    const store = useProcessesStore();

    await store.scanNow();

    expect(store.snapshotMeta?.processCount).toBe(1);
    expect(store.rowOrder).toEqual(['100:111']);
    expect(store.rows['100:111']).toMatchObject({
      classification: 'userApplication',
      applicationName: 'Demo App',
    });
    expect(store.associationsStatus).toBe('available');
    expect(store.knownUsers).toEqual(['ryan']);
  });

  it('refreshes rows in place keyed by pid and start time', async () => {
    const scan = vi.spyOn(ProcessService, 'scan').mockResolvedValue(scanView([sample({ cpuPercent: 5 })]));
    const store = useProcessesStore();
    await store.scanNow();
    const before = store.rows['100:111'];

    scan.mockResolvedValue(
      scanView([sample({ cpuPercent: 42 }), sample({ pid: 200, name: 'fresh', startedAtMs: 222 })])
    );
    await store.scanNow();

    expect(store.rows['100:111']).toBe(before);
    expect(before.sample.cpuPercent).toBe(42);
    expect(store.rows['200:222']?.sample.name).toBe('fresh');
    expect(store.rowOrder).toEqual(['100:111', '200:222']);
  });

  it('prunes vanished rows, their selections, and an open details drawer', async () => {
    const scan = vi
      .spyOn(ProcessService, 'scan')
      .mockResolvedValue(scanView([sample(), sample({ pid: 200, name: 'gone', startedAtMs: 222 })]));
    const store = useProcessesStore();
    await store.scanNow();
    store.toggleRowSelection('100:111');
    store.toggleRowSelection('200:222');
    store.openDetails('200:222');

    scan.mockResolvedValue(scanView([sample()]));
    await store.scanNow();

    expect(store.rows['200:222']).toBeUndefined();
    expect(store.selectedKeys).toEqual(['100:111']);
    expect(store.detailsKey).toBeNull();
  });

  it('surfaces an initial failure and keeps later refresh failures out of the global error state', async () => {
    const commandError = { code: 'operationFailed', details: { operation: 'scan_processes' }, retryable: true };
    const scan = vi.spyOn(ProcessService, 'scan').mockRejectedValue(commandError);
    vi.spyOn(LoggerService, 'warn').mockImplementation(() => undefined);
    const store = useProcessesStore();
    const appStore = useAppStore();

    await store.scanNow();
    expect(store.loadFailed).toBe(true);
    expect(appStore.errorCode).toBe('operationFailed');

    scan.mockResolvedValue(scanView([sample()]));
    await store.scanNow();
    expect(store.loadFailed).toBe(false);
    appStore.clearError();

    scan.mockRejectedValue(commandError);
    await store.scanNow();
    expect(appStore.errorCode).toBeNull();
    expect(store.rows['100:111']).toBeDefined();
    expect(LoggerService.warn).toHaveBeenCalledWith('processes', 'process_scan_failed', expect.anything());
  });

  it('maps and clamps filter text for the backend scan filter', async () => {
    vi.spyOn(ProcessService, 'scan').mockResolvedValue(scanView([sample()]));
    const store = useProcessesStore();

    store.filterName = '  chrome  ';
    store.filterUser = 'ryan';
    expect(store.currentScanFilter()).toEqual({ nameContains: 'chrome', user: 'ryan' });

    store.filterName = 'x'.repeat(500);
    const clamped = store.currentScanFilter().nameContains ?? '';
    expect(new TextEncoder().encode(clamped).length).toBeLessThanOrEqual(128);
  });
});

describe('processes store live refresh loop', () => {
  it('scans immediately, then on the interval, until stopped', async () => {
    vi.useFakeTimers();
    const scan = vi.spyOn(ProcessService, 'scan').mockResolvedValue(scanView([sample()]));
    const store = useProcessesStore();

    store.startLiveUpdates();
    await vi.advanceTimersByTimeAsync(0);
    expect(scan).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(2500);
    expect(scan).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(2500);
    expect(scan).toHaveBeenCalledTimes(3);

    store.stopLiveUpdates();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(scan).toHaveBeenCalledTimes(3);
  });

  it('pauses while the browser is hidden or unfocused and rescans on return', async () => {
    vi.useFakeTimers();
    const scan = vi.spyOn(ProcessService, 'scan').mockResolvedValue(scanView([sample()]));
    const store = useProcessesStore();

    store.startLiveUpdates();
    await vi.advanceTimersByTimeAsync(0);
    store.setBrowserActive(false);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(scan).toHaveBeenCalledTimes(1);

    store.setBrowserActive(true);
    await vi.advanceTimersByTimeAsync(0);
    expect(scan).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(2500);
    expect(scan).toHaveBeenCalledTimes(3);
  });

  it('debounces filter changes into a fresh scan', async () => {
    vi.useFakeTimers();
    const scan = vi.spyOn(ProcessService, 'scan').mockResolvedValue(scanView([sample()]));
    const store = useProcessesStore();

    store.startLiveUpdates();
    await vi.advanceTimersByTimeAsync(0);
    store.setNameFilter('chr');
    store.setNameFilter('chrome');
    await vi.advanceTimersByTimeAsync(249);
    expect(scan).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(scan).toHaveBeenCalledTimes(2);
    expect(scan).toHaveBeenLastCalledWith({ nameContains: 'chrome', user: null });
  });
});

describe('processes store kill flow', () => {
  it('never selects critical system rows', async () => {
    vi.spyOn(ProcessService, 'scan').mockResolvedValue(
      scanView([sample()], { classifications: { 100: 'criticalSystem' } })
    );
    const store = useProcessesStore();
    await store.scanNow();

    store.toggleRowSelection('100:111');
    expect(store.selectedKeys).toEqual([]);
    expect(store.selectedPids).toEqual([]);
  });

  it('prepares a plan for the selection and exposes per-item refusals', async () => {
    vi.spyOn(ProcessService, 'scan').mockResolvedValue(
      scanView([sample(), sample({ pid: 200, name: 'daemon', startedAtMs: 222 })], {
        classifications: { 200: 'systemService' },
      })
    );
    const store = useProcessesStore();
    await store.scanNow();
    const prepared = plan([
      planItem(),
      planItem({
        pid: 200,
        name: 'daemon',
        startedAtMs: 222,
        classification: 'systemService',
        decision: { refused: 'ownedByOtherUser' },
      }),
    ]);
    const prepare = vi.spyOn(ProcessService, 'prepareEnd').mockResolvedValue(prepared);

    store.toggleRowSelection('100:111');
    store.toggleRowSelection('200:222');
    await store.prepareEnd();

    expect(prepare).toHaveBeenCalledWith([100, 200]);
    expect(store.endPlan?.items[0].decision).toBe('allowed');
    expect(store.endPlan?.items[1].decision).toEqual({ refused: 'ownedByOtherUser' });
    expect(store.endMode).toBe('graceful');
  });

  it('executes a confirmed plan, keeps only remaining pids selected, and rescans', async () => {
    const scan = vi
      .spyOn(ProcessService, 'scan')
      .mockResolvedValue(scanView([sample(), sample({ pid: 200, name: 'stubborn', startedAtMs: 222 })]));
    const store = useProcessesStore();
    await store.scanNow();
    const prepared = plan([planItem(), planItem({ pid: 200, name: 'stubborn', startedAtMs: 222 })]);
    vi.spyOn(ProcessService, 'prepareEnd').mockResolvedValue(prepared);
    const execute = vi.spyOn(ProcessService, 'executeEnd').mockResolvedValue(
      endResult({
        requestedCount: 2,
        endedCount: 1,
        failedCount: 1,
        remainingPids: [200],
        items: [
          { pid: 100, name: 'demo', status: 'ended', refusal: null },
          { pid: 200, name: 'stubborn', status: 'stillRunning', refusal: null },
        ],
      })
    );

    store.toggleRowSelection('100:111');
    store.toggleRowSelection('200:222');
    await store.prepareEnd();
    await store.executeEnd(true);

    expect(execute).toHaveBeenCalledWith(prepared, 'graceful', true);
    expect(store.endPlan).toBeNull();
    expect(store.endResult?.remainingPids).toEqual([200]);
    expect(store.selectedKeys).toEqual(['200:222']);
    // One scan for the initial load and one forced refresh after execution.
    expect(scan).toHaveBeenCalledTimes(2);
  });

  it('forwards the explicit force mode to execution', async () => {
    vi.spyOn(ProcessService, 'scan').mockResolvedValue(scanView([sample()]));
    const store = useProcessesStore();
    await store.scanNow();
    const prepared = plan([planItem()]);
    vi.spyOn(ProcessService, 'prepareEnd').mockResolvedValue(prepared);
    const execute = vi.spyOn(ProcessService, 'executeEnd').mockResolvedValue(endResult({ mode: 'force' }));

    store.toggleRowSelection('100:111');
    await store.prepareEnd();
    store.setEndMode('force');
    await store.executeEnd(true);

    expect(execute).toHaveBeenCalledWith(prepared, 'force', true);
  });

  it('drops a rejected plan so the next attempt re-prepares', async () => {
    vi.spyOn(ProcessService, 'scan').mockResolvedValue(scanView([sample()]));
    vi.spyOn(LoggerService, 'warn').mockImplementation(() => undefined);
    const store = useProcessesStore();
    await store.scanNow();
    const prepared = plan([planItem()]);
    vi.spyOn(ProcessService, 'prepareEnd').mockResolvedValue(prepared);
    // Core's confirmation gate mirrors into a typed invalid-input refusal.
    vi.spyOn(ProcessService, 'executeEnd').mockRejectedValue({
      code: 'invalidInput',
      details: { operation: 'execute_process_end' },
      retryable: false,
    });

    store.toggleRowSelection('100:111');
    await store.prepareEnd();
    await store.executeEnd(false);

    expect(ProcessService.executeEnd).toHaveBeenCalledWith(prepared, 'graceful', false);
    expect(store.endPlan).toBeNull();
    expect(store.endResult).toBeNull();
    expect(useAppStore().errorCode).toBe('invalidInput');
    expect(LoggerService.warn).toHaveBeenCalledWith('processes', 'process_end_execute_failed', expect.anything());
  });

  it('cancels a prepared plan without executing it', async () => {
    vi.spyOn(ProcessService, 'scan').mockResolvedValue(scanView([sample()]));
    const store = useProcessesStore();
    await store.scanNow();
    vi.spyOn(ProcessService, 'prepareEnd').mockResolvedValue(plan([planItem()]));
    const execute = vi.spyOn(ProcessService, 'executeEnd').mockResolvedValue(endResult());

    store.toggleRowSelection('100:111');
    await store.prepareEnd();
    store.cancelEndPlan();

    expect(store.endPlan).toBeNull();
    expect(execute).not.toHaveBeenCalled();
  });
});

describe('processes store Ask-AI handoff', () => {
  it('seeds the chat composer with structured process facts and navigates to chat', async () => {
    vi.spyOn(ProcessService, 'scan').mockResolvedValue(
      scanView([sample({ writeBps: 2048 })], {
        classifications: { 100: 'userApplication' },
        applications: { 100: 'Demo App' },
      })
    );
    const store = useProcessesStore();
    await store.scanNow();

    store.askAiAboutProcess('100:111');

    const chatStore = useChatStore();
    const text = chatStore.composerSeed?.text ?? '';
    expect(text).toContain('name: demo');
    expect(text).toContain('pid: 100');
    expect(text).toContain('user: ryan');
    expect(text).toContain('classification: userApplication');
    expect(text).toContain('associated application: Demo App');
    expect(text).toContain('executable: /usr/bin/demo');
    expect(text).toContain('memory (rss): 4096 bytes');
    expect(text).toContain('disk write rate: 2048 B/s');
    expect(useAppStore().currentPage).toBe('chat');
  });

  it('redacts a missing executable through its typed absence code', async () => {
    vi.spyOn(ProcessService, 'scan').mockResolvedValue(
      scanView([sample({ executablePath: null, executablePathAbsence: 'accessDenied' })])
    );
    const store = useProcessesStore();
    await store.scanNow();

    store.askAiAboutProcess('100:111');

    const text = useChatStore().composerSeed?.text ?? '';
    expect(text).toContain('executable: unavailable (accessDenied)');
    expect(text).not.toContain('/usr/bin/demo');
  });

  it('seeds the disk-busy question with the top writers first', async () => {
    vi.spyOn(ProcessService, 'scan').mockResolvedValue(
      scanView([
        sample({ pid: 100, name: 'slow-writer', writeBps: 100 }),
        sample({ pid: 200, name: 'fast-writer', writeBps: 9_000_000, startedAtMs: 222 }),
        sample({ pid: 300, name: 'no-io', writeBps: null, startedAtMs: 333 }),
      ])
    );
    const store = useProcessesStore();
    await store.scanNow();

    store.askAiAboutDiskActivity();

    const chatStore = useChatStore();
    const text = chatStore.composerSeed?.text ?? '';
    expect(text).toContain('Why is my disk busy?');
    expect(text).toContain('fast-writer');
    expect(text).toContain('slow-writer');
    expect(text.indexOf('fast-writer')).toBeLessThan(text.indexOf('slow-writer'));
    expect(text).not.toContain('no-io');
    expect(useAppStore().currentPage).toBe('chat');
  });

  it('does nothing without a snapshot', () => {
    const store = useProcessesStore();
    store.askAiAboutDiskActivity();
    expect(useChatStore().composerSeed).toBeNull();
  });
});

describe('chat store composer seeding', () => {
  it('keeps the newest seed and clears it when consumed', () => {
    const chatStore = useChatStore();

    chatStore.seedComposer('  first  ');
    expect(chatStore.composerSeed).toEqual({ text: 'first', revision: 1 });
    chatStore.seedComposer('second');
    expect(chatStore.composerSeed).toEqual({ text: 'second', revision: 2 });

    chatStore.consumeComposerSeed();
    expect(chatStore.composerSeed).toBeNull();
  });
});
