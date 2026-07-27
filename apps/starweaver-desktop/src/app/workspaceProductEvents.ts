import type { SafeHostEvent } from "../generated/host/types";
import type {
  RunSummary,
  RunTranscript,
  RunTranscripts,
  TranscriptMessage,
} from "./workspaceProductTypes";

export const ACTIVE_RUN_STATES = new Set<RunSummary["status"]>([
  "queued",
  "starting",
  "running",
  "waiting",
]);

export function replaceById<T>(
  entries: readonly T[],
  next: T,
  id: (entry: T) => string,
): readonly T[] {
  const index = entries.findIndex((entry) => id(entry) === id(next));
  if (index === -1) return [next, ...entries];
  return entries.map((entry, entryIndex) => (entryIndex === index ? next : entry));
}

export function replaceRun(runs: readonly RunSummary[], run: RunSummary): readonly RunSummary[] {
  const existing = runs.findIndex((candidate) => candidate.runId === run.runId);
  if (existing === -1) return [...runs, run];
  return runs.map((candidate, index) => {
    if (index !== existing) return candidate;
    return {
      ...candidate,
      ...run,
      inputPreview: run.inputPreview ?? candidate.inputPreview,
      outputPreview: run.outputPreview ?? candidate.outputPreview,
    };
  });
}

export function applyRunHostEvent(
  runs: readonly RunSummary[],
  event: SafeHostEvent,
): readonly RunSummary[] {
  const payload = event.delivery.record.event;
  if (payload.kind === "run_changed") return replaceRun(runs, payload.run);
  if (payload.kind === "output_available") {
    return runs.map((run) =>
      run.runId === payload.runId ? { ...run, outputPreview: payload.preview } : run,
    );
  }
  return runs;
}

function isNewerSequence(candidate: string, previous: string | undefined): boolean {
  if (previous === undefined) return true;
  try {
    return BigInt(candidate) > BigInt(previous);
  } catch {
    return false;
  }
}

export function applyTranscriptHostEvent(
  transcripts: RunTranscripts,
  event: SafeHostEvent,
): RunTranscripts {
  const payload = event.delivery.record.event;
  if (payload.kind !== "transcript_changed") return transcripts;
  const current = transcripts[payload.runId];
  if (!isNewerSequence(payload.transcriptSequence, current?.lastSequence)) return transcripts;

  const update = payload.update;
  const existingMessage = current?.messages[update.messageId];
  const messageOrder =
    existingMessage === undefined
      ? [...(current?.messageOrder ?? []), update.messageId]
      : (current?.messageOrder ?? []);
  let message: TranscriptMessage;
  if (update.kind === "text_appended") {
    message = {
      messageId: update.messageId,
      text: `${existingMessage?.text ?? ""}${update.delta}`,
      lifecycle: existingMessage?.lifecycle ?? "streaming",
      lastSequence: payload.transcriptSequence,
    };
  } else if (update.kind === "message_finished") {
    message = {
      messageId: update.messageId,
      text: existingMessage?.text ?? "",
      lifecycle: "finished",
      lastSequence: payload.transcriptSequence,
    };
  } else {
    message = {
      messageId: update.messageId,
      text: existingMessage?.text ?? "",
      lifecycle: existingMessage?.lifecycle ?? "streaming",
      lastSequence: payload.transcriptSequence,
    };
  }
  return {
    ...transcripts,
    [payload.runId]: {
      lastSequence: payload.transcriptSequence,
      messageOrder,
      messages: { ...(current?.messages ?? {}), [update.messageId]: message },
    },
  };
}

export function transcriptText(transcript: RunTranscript | undefined): string | undefined {
  if (transcript === undefined) return undefined;
  const text = transcript.messageOrder.map((id) => transcript.messages[id]?.text ?? "").join("");
  return text.length > 0 ? text : undefined;
}
