// What a card says and which actions it offers, derived from its status.

import type { DownloadCard } from "../types";
import { fmtWhen } from "./format";

export function statusText(card: DownloadCard): string {
  const pos = card.queuePosition && card.queuePosition > 0 ? ` · #${card.queuePosition}` : "";
  switch (card.status) {
    case "downloading":
      return "Downloading";
    case "paused":
      return "Paused";
    case "muxing":
      return "Muxing (FFmpeg)…";
    case "complete":
      return "Completed";
    case "error":
      return "Error";
    case "canceled":
      return "Canceled";
    case "extracting":
      return "Extracting streams…";
    case "starting":
      return "Connecting…";
    case "queued":
      return `Queued${pos}`;
    case "held":
      return `Paused in queue${pos}`;
    case "scheduled":
      return card.startAt ? `Scheduled for ${fmtWhen(card.startAt)}` : "Scheduled";
    case "interrupted":
      return "Interrupted — resumable";
    default:
      return card.status;
  }
}

export type CardAction =
  | "pause"
  | "resume"
  | "retry"
  | "cancel"
  | "open"
  | "reveal"
  | "remove"
  | "reorder";

/** Buttons shown on a card, in display order. */
export function cardActions(card: DownloadCard): CardAction[] {
  switch (card.status) {
    case "downloading":
      return ["pause", "cancel"];
    case "paused":
      return ["resume", "cancel"];
    case "queued":
    case "scheduled":
      return ["reorder", "pause", "cancel"];
    case "held":
      return ["reorder", "resume", "cancel"];
    case "complete":
      return ["open", "reveal", "remove"];
    case "interrupted":
    case "error":
    case "canceled":
      return ["retry", "reveal", "remove"];
    default:
      // starting / extracting / muxing
      return ["cancel"];
  }
}

/** A restored download whose header values weren't saved can't be resumed. */
export function missingHeaders(card: DownloadCard): boolean {
  return (card.headerNames?.length ?? 0) > 0 && card.status === "interrupted";
}
