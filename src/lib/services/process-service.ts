import { invoke } from '@tauri-apps/api/core';

import type {
  ProcessEndMode,
  ProcessEndPlan,
  ProcessEndResult,
  ProcessScanFilter,
  ProcessScanView,
} from '@/lib/models/process';

export class ProcessService {
  /** One call performs the full two-sample scan (~500 ms) plus Core projections. */
  static scan(filter: ProcessScanFilter): Promise<ProcessScanView> {
    return invoke<ProcessScanView>('scan_processes', { filter });
  }

  static prepareEnd(pids: number[]): Promise<ProcessEndPlan> {
    return invoke<ProcessEndPlan>('prepare_process_end', { pids });
  }

  /** `confirmed` mirrors the Core gate; the confirmation dialog is the only caller passing true. */
  static executeEnd(plan: ProcessEndPlan, mode: ProcessEndMode, confirmed: boolean): Promise<ProcessEndResult> {
    return invoke<ProcessEndResult>('execute_process_end', { plan, mode, confirmed });
  }
}
