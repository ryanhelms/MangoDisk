import { PROCESS_CLASSIFICATION_ORDER, type ProcessClassification, type ProcessTree } from '@/lib/models/process';
import type { ProcessRow } from '@/stores/processes-store';

/**
 * Presentation helpers for the processes page. Everything here is pure: the
 * store owns scanning and selection, and these functions only project rows
 * into sorted or tree-flattened display order.
 */

export const PROCESS_SORT_KEYS = [
  'name',
  'pid',
  'user',
  'cpu',
  'rss',
  'readRate',
  'writeRate',
  'openFiles',
  'application',
  'classification',
] as const;
export type ProcessSortKey = (typeof PROCESS_SORT_KEYS)[number];
export type ProcessSortDirection = 'asc' | 'desc';

export function nextProcessSort(
  key: ProcessSortKey,
  activeKey: ProcessSortKey,
  direction: ProcessSortDirection
): { key: ProcessSortKey; direction: ProcessSortDirection } {
  if (key !== activeKey)
    return { key, direction: key === 'name' || key === 'user' || key === 'application' ? 'asc' : 'desc' };
  return { key, direction: direction === 'asc' ? 'desc' : 'asc' };
}

function classificationRank(classification: ProcessClassification): number {
  const index = PROCESS_CLASSIFICATION_ORDER.indexOf(classification);
  return index === -1 ? PROCESS_CLASSIFICATION_ORDER.length : index;
}

function compareText(left: string, right: string, locale: string): number {
  return left.localeCompare(right, locale, { sensitivity: 'base' });
}

/** Missing metric values always rank below measured ones, in both directions. */
function compareMetric(left: number | null, right: number | null): number {
  if (left === null && right === null) return 0;
  if (left === null) return 1;
  if (right === null) return -1;
  return left - right;
}

function compareRows(left: ProcessRow, right: ProcessRow, key: ProcessSortKey, locale: string): number {
  switch (key) {
    case 'name':
      return compareText(left.sample.name, right.sample.name, locale);
    case 'pid':
      return left.sample.pid - right.sample.pid;
    case 'user':
      return compareText(left.sample.ownerName ?? '', right.sample.ownerName ?? '', locale);
    case 'cpu':
      return compareMetric(left.sample.cpuPercent, right.sample.cpuPercent);
    case 'rss':
      return left.sample.rssBytes - right.sample.rssBytes;
    case 'readRate':
      return compareMetric(left.sample.readBps, right.sample.readBps);
    case 'writeRate':
      return compareMetric(left.sample.writeBps, right.sample.writeBps);
    case 'openFiles':
      return compareMetric(left.sample.openFileCount ?? null, right.sample.openFileCount ?? null);
    case 'application':
      return compareText(left.applicationName ?? '', right.applicationName ?? '', locale);
    case 'classification':
      return classificationRank(left.classification) - classificationRank(right.classification);
  }
}

/** Stable sort: equal values keep pid order so refresh ticks never shuffle rows. */
export function sortProcessRows(
  rows: ProcessRow[],
  key: ProcessSortKey,
  direction: ProcessSortDirection,
  locale: string
): ProcessRow[] {
  const sign = direction === 'asc' ? 1 : -1;
  return [...rows].sort((left, right) => {
    const order = compareRows(left, right, key, locale);
    return order !== 0 ? order * sign : left.sample.pid - right.sample.pid;
  });
}

export interface ProcessTreeRowPosition {
  key: string;
  depth: number;
}

/**
 * Flattens the Core process tree into display order. The synthetic pid-0 root
 * is skipped (its children render at depth 0), and a visited set guards
 * against platform cycles so rendering can never recurse forever.
 */
export function flattenProcessTree(
  tree: ProcessTree,
  rows: Record<string, ProcessRow>,
  rowOrder: string[]
): ProcessTreeRowPosition[] {
  const keyByPid = new Map<number, string>();
  for (const key of rowOrder) {
    const row = rows[key];
    if (row) keyByPid.set(row.sample.pid, key);
  }
  const positions: ProcessTreeRowPosition[] = [];
  const visited = new Set<number>();
  const roots = tree.roots.flatMap(pid => {
    const node = tree.nodes[pid];
    return node?.synthetic ? node.children : [pid];
  });
  const walk = (pid: number, depth: number) => {
    if (visited.has(pid)) return;
    visited.add(pid);
    const key = keyByPid.get(pid);
    if (key) positions.push({ key, depth });
    const node = tree.nodes[pid];
    for (const childPid of node?.children ?? []) walk(childPid, depth + 1);
  };
  for (const pid of roots) walk(pid, 0);
  // Rows missing from the tree (for example a race inside the snapshot) stay
  // visible at the end instead of disappearing in tree mode.
  for (const key of rowOrder) {
    const row = rows[key];
    if (row && !visited.has(row.sample.pid)) positions.push({ key, depth: 0 });
  }
  return positions;
}
