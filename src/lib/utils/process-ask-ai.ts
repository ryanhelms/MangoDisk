import type { ProcessClassification, ProcessSample } from '@/lib/models/process';

/**
 * Builds the factual chat seeds for the processes page Ask-AI handoff. The
 * messages carry structured English facts (stable typed codes and raw numbers)
 * instead of localized prose, so the agent receives the same evidence in every
 * UI language.
 */

export const PROCESS_ASK_AI_TOP_WRITER_COUNT = 5;

export interface ProcessAskAiEntry {
  sample: ProcessSample;
  classification: ProcessClassification;
  applicationName: string | null;
}

function formatIso(timestampMs: number): string {
  return timestampMs > 0 ? new Date(timestampMs).toISOString() : 'unknown';
}

function formatRate(bps: number | null, absence: string | null): string {
  if (bps !== null) return `${Math.round(bps)} B/s`;
  return absence ? `unavailable (${absence})` : 'not measured yet';
}

function formatPercent(cpuPercent: number | null): string {
  return cpuPercent === null ? 'not measured yet' : `${cpuPercent.toFixed(1)}% of total machine capacity`;
}

function describeEntry(entry: ProcessAskAiEntry, includeIdentity: boolean): string[] {
  const { sample, classification, applicationName } = entry;
  const lines = [
    `name: ${sample.name}`,
    `pid: ${sample.pid}`,
    `user: ${sample.ownerName ?? (sample.ownerUid === null ? 'unknown' : `uid ${sample.ownerUid}`)}`,
    `owned by the current user: ${sample.ownedByCurrentUser === null ? 'unknown' : sample.ownedByCurrentUser ? 'yes' : 'no'}`,
    `classification: ${classification}`,
  ];
  if (applicationName) lines.push(`associated application: ${applicationName}`);
  if (includeIdentity) {
    lines.push(
      sample.executablePath === null
        ? `executable: unavailable (${sample.executablePathAbsence ?? 'unknown'})`
        : `executable: ${sample.executablePath}`,
      `state: ${sample.state}`,
      `threads: ${sample.threadCount}`,
      `started at: ${formatIso(sample.startedAtMs)}`
    );
  }
  lines.push(
    `cpu: ${formatPercent(sample.cpuPercent)}`,
    `memory (rss): ${sample.rssBytes} bytes`,
    `disk read rate: ${formatRate(sample.readBps, sample.ioAbsence)}`,
    `disk write rate: ${formatRate(sample.writeBps, sample.ioAbsence)}`,
    `open files: ${sample.openFileCount ?? `unavailable (${sample.openFilesAbsence ?? 'unknown'})`}`
  );
  return lines;
}

/** Seed for "Ask AI about this process": one process with its typed facts. */
export function buildProcessAnalysisSeed(entry: ProcessAskAiEntry): string {
  return [
    'Please analyze this process observed by my MangoDisk process monitor. Explain what it likely is, whether the reported load looks normal, and safe next steps. Do not end any process without my explicit confirmation.',
    '',
    'Process facts:',
    ...describeEntry(entry, true).map(line => `- ${line}`),
  ].join('\n');
}

/** Seed for "Why is my disk busy?": the highest measured disk writers. */
export function buildDiskBusySeed(input: {
  capturedAtMs: number;
  processCount: number;
  writers: ProcessAskAiEntry[];
}): string {
  const lines = [
    'Why is my disk busy? Below are the top disk writers from a live MangoDisk process snapshot. Explain which of these are expected, what each one likely writes, and how to reduce disk write load safely. Do not end any process without my explicit confirmation.',
    '',
    `Snapshot captured at: ${formatIso(input.capturedAtMs)}`,
    `Processes in snapshot: ${input.processCount}`,
  ];
  if (!input.writers.length) {
    lines.push('No process reported a measurable disk write rate in this snapshot.');
  } else {
    lines.push('Top disk writers:');
    input.writers.forEach((entry, index) => {
      lines.push(`${index + 1}. ${describeEntry(entry, false).join('; ')}`);
    });
  }
  return lines.join('\n');
}
