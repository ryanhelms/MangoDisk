/**
 * Process-analysis protocol between the Core `processes` domain and this
 * frontend. Field names mirror the serde `camelCase` shapes in
 * `mangodisk-core/src/processes`; enum values are the stable typed codes the
 * adapter forwards. The locale-key maps keep enum-driven labels as literal,
 * checkable translation consumers.
 */

/** Typed reason a metric is absent, serialized by mangodisk-platform. */
export type ProcessMetricAbsence = 'unsupported' | 'accessDenied' | 'notApplicable' | 'notAvailable';

export type ProcessState = 'running' | 'sleeping' | 'idle' | 'stopped' | 'zombie' | 'dead' | 'unknown';

export type ProcessClassification = 'criticalSystem' | 'systemService' | 'userApplication' | 'userBackground';

export interface ProcessScanFilter {
  nameContains?: string | null;
  user?: string | null;
  minRssBytes?: number | null;
}

export interface ProcessSample {
  pid: number;
  ppid: number;
  name: string;
  executablePath: string | null;
  executablePathAbsence: ProcessMetricAbsence | null;
  ownerUid: number | null;
  ownerName: string | null;
  ownedByCurrentUser: boolean | null;
  state: ProcessState;
  threadCount: number;
  cpuUserTicks: number;
  cpuKernelTicks: number;
  cpuTicksPerSecond: number;
  /** Null for processes first seen in the second sample; never a lifetime average. */
  cpuPercent: number | null;
  rssBytes: number;
  ioReadBytes: number | null;
  ioWriteBytes: number | null;
  ioAbsence: ProcessMetricAbsence | null;
  readBps: number | null;
  writeBps: number | null;
  openFileCount: number | null;
  openFilesAbsence: ProcessMetricAbsence | null;
  /** Unix epoch milliseconds; 0 means the platform could not determine it. */
  startedAtMs: number;
}

export interface ProcessSnapshot {
  schemaVersion: number;
  snapshotId: string;
  capturedAtMs: number;
  sampleIntervalMs: number;
  cpuTicksPerSecond: number;
  logicalCpuCount: number;
  newProcessCount: number;
  exitedProcessCount: number;
  processes: ProcessSample[];
}

export interface ProcessTreeNode {
  pid: number;
  ppid: number;
  name: string;
  synthetic: boolean;
  children: number[];
}

export interface ProcessTree {
  /** Numeric pids serialize as JSON object keys; index with the pid value. */
  nodes: Record<number, ProcessTreeNode>;
  roots: number[];
}

export type ProcessAssociationInventoryStatus = 'available' | 'unavailable';

export interface ProcessApplicationMatch {
  pid: number;
  applicationIdentifier: string | null;
  applicationName: string | null;
}

export interface ProcessApplicationAssociations {
  inventoryStatus: ProcessAssociationInventoryStatus;
  matches: ProcessApplicationMatch[];
}

export interface ProcessClassificationEntry {
  pid: number;
  classification: ProcessClassification;
}

/** Composed adapter response for `scan_processes`. */
export interface ProcessScanView {
  snapshot: ProcessSnapshot;
  tree: ProcessTree;
  associations: ProcessApplicationAssociations;
  classifications: ProcessClassificationEntry[];
}

export type ProcessEndMode = 'graceful' | 'force';

export type ProcessEndRefusal = 'processNotFound' | 'ownedByOtherUser' | 'ownershipUnknown';

/** Serde external tagging: a unit variant is a bare string, a payload variant an object. */
export type ProcessEndDecision = 'allowed' | { refused: ProcessEndRefusal };

export interface ProcessEndPlanItem {
  pid: number;
  name: string;
  startedAtMs: number;
  classification: ProcessClassification;
  decision: ProcessEndDecision;
}

export interface ProcessEndPlan {
  schemaVersion: number;
  planId: string;
  issuedAtMs: number;
  items: ProcessEndPlanItem[];
}

export type ProcessEndItemStatus =
  | 'ended'
  | 'endedAfterForce'
  | 'alreadyExited'
  | 'stillRunning'
  | 'permissionDenied'
  | 'unsupported'
  | 'identityChanged'
  | 'refused'
  | 'failed';

export interface ProcessEndItemResult {
  pid: number;
  name: string;
  status: ProcessEndItemStatus;
  refusal: ProcessEndRefusal | null;
}

export interface ProcessEndResult {
  planId: string;
  mode: ProcessEndMode;
  requestedCount: number;
  endedCount: number;
  failedCount: number;
  /** Final authority over every per-item status: processes still alive. */
  remainingPids: number[];
  items: ProcessEndItemResult[];
  elapsedMs: number;
}

/** Snapshot schema version this frontend renders; mismatches fail closed. */
export const PROCESS_SNAPSHOT_SCHEMA_VERSION = 1 as const;
export const PROCESS_END_PLAN_SCHEMA_VERSION = 1 as const;

/** Live refresh cadence for the processes page; the scan itself takes ~500 ms. */
export const PROCESS_LIVE_REFRESH_INTERVAL_MS = 2500;

/** Severity order for grouping kill plans and ranking the classification column. */
export const PROCESS_CLASSIFICATION_ORDER: readonly ProcessClassification[] = [
  'criticalSystem',
  'systemService',
  'userApplication',
  'userBackground',
];

/** Stable row identity: a pid alone cannot survive pid reuse across scans. */
export function processRowKey(pid: number, startedAtMs: number): string {
  return `${pid}:${startedAtMs}`;
}

export function processEndDecisionRefusal(decision: ProcessEndDecision): ProcessEndRefusal | null {
  return typeof decision === 'string' ? null : decision.refused;
}

/** Locale key per classification; literal values keep translations checkable. */
export const PROCESS_CLASSIFICATION_LABEL_KEYS: Record<ProcessClassification, string> = {
  criticalSystem: 'processes.classifications.criticalSystem',
  systemService: 'processes.classifications.systemService',
  userApplication: 'processes.classifications.userApplication',
  userBackground: 'processes.classifications.userBackground',
};

/** Locale key per classification meaning, shown in the details drawer. */
export const PROCESS_CLASSIFICATION_DESCRIPTION_KEYS: Record<ProcessClassification, string> = {
  criticalSystem: 'processes.classificationDescriptions.criticalSystem',
  systemService: 'processes.classificationDescriptions.systemService',
  userApplication: 'processes.classificationDescriptions.userApplication',
  userBackground: 'processes.classificationDescriptions.userBackground',
};

/** Locale key per lifecycle state. */
export const PROCESS_STATE_LABEL_KEYS: Record<ProcessState, string> = {
  running: 'processes.states.running',
  sleeping: 'processes.states.sleeping',
  idle: 'processes.states.idle',
  stopped: 'processes.states.stopped',
  zombie: 'processes.states.zombie',
  dead: 'processes.states.dead',
  unknown: 'processes.states.unknown',
};

/** Locale key per typed metric absence. */
export const PROCESS_METRIC_ABSENCE_LABEL_KEYS: Record<ProcessMetricAbsence, string> = {
  unsupported: 'processes.metricAbsence.unsupported',
  accessDenied: 'processes.metricAbsence.accessDenied',
  notApplicable: 'processes.metricAbsence.notApplicable',
  notAvailable: 'processes.metricAbsence.notAvailable',
};

/** Locale key per kill-plan refusal reason. */
export const PROCESS_END_REFUSAL_LABEL_KEYS: Record<ProcessEndRefusal, string> = {
  processNotFound: 'processes.end.refusals.processNotFound',
  ownedByOtherUser: 'processes.end.refusals.ownedByOtherUser',
  ownershipUnknown: 'processes.end.refusals.ownershipUnknown',
};

/** Locale key per execution outcome status. */
export const PROCESS_END_ITEM_STATUS_LABEL_KEYS: Record<ProcessEndItemStatus, string> = {
  ended: 'processes.end.resultStatuses.ended',
  endedAfterForce: 'processes.end.resultStatuses.endedAfterForce',
  alreadyExited: 'processes.end.resultStatuses.alreadyExited',
  stillRunning: 'processes.end.resultStatuses.stillRunning',
  permissionDenied: 'processes.end.resultStatuses.permissionDenied',
  unsupported: 'processes.end.resultStatuses.unsupported',
  identityChanged: 'processes.end.resultStatuses.identityChanged',
  refused: 'processes.end.resultStatuses.refused',
  failed: 'processes.end.resultStatuses.failed',
};

/** Locale key per end mode; shared with the history record rendering. */
export const PROCESS_END_MODE_LABEL_KEYS: Record<ProcessEndMode, string> = {
  graceful: 'processes.end.modes.graceful',
  force: 'processes.end.modes.force',
};
