// Pure card model: every backend event is folded into dashboard state here.
// No React, no Tauri — unit-tested in cards.test.ts.

import type {
  DownloadCard,
  JobRecord,
  ProgressPayload,
  QueueStatus,
  YoutubeStatus,
} from "../types";

export interface CardsState {
  /** Card ids in display (insertion) order. */
  order: string[];
  cards: Record<string, DownloadCard>;
}

export const emptyCards: CardsState = { order: [], cards: {} };

export type CardsAction =
  | { type: "progress"; payload: ProgressPayload }
  | { type: "youtube"; payload: YoutubeStatus }
  | { type: "queue"; payload: QueueStatus }
  | { type: "removed"; ids: string[] }
  | { type: "history"; records: JobRecord[] }
  | { type: "seed"; card: DownloadCard };

// A YouTube parent id is `<uuid>`; its jobs are `<uuid>::video|audio`.
export function groupId(id: string): string {
  return id.split("::")[0];
}

export function streamLabel(id: string): "video" | "audio" | null {
  if (id.endsWith("::video")) return "video";
  if (id.endsWith("::audio")) return "audio";
  return null;
}

export function blankCard(id: string, fileName: string, status: string): DownloadCard {
  return {
    id,
    fileName,
    status,
    total: 0,
    downloaded: 0,
    speed: 0,
    progress: 0,
    eta: -1,
    connections: 0,
    isYoutube: false,
    speedLimit: 0,
    jobs: {},
  };
}

export function cardFromRecord(r: JobRecord): DownloadCard {
  const waiting = r.status === "queued" || r.status === "scheduled";
  return {
    ...blankCard(r.id, r.fileName, waiting && r.held ? "held" : r.status),
    total: r.total,
    downloaded: r.downloaded,
    progress: r.total > 0 ? Math.min(100, (r.downloaded / r.total) * 100) : 0,
    connections: r.connections,
    isYoutube: r.kind === "youtube",
    speedLimit: r.speedLimit,
    error: r.error,
    startAt: r.startAt,
    headerNames: r.headerNames,
    priority: r.priority,
  };
}

export interface Aggregate {
  total: number;
  downloaded: number;
  speed: number;
  progress: number;
  eta: number;
  connections: number;
  speedLimit: number;
  status: string;
  sha256?: string | null;
  verified?: boolean | null;
}

/**
 * Merge a card's engine jobs (1 for a file; video + audio for YouTube) into
 * one set of numbers. Status: error wins, then canceled, then paused;
 * complete only when every job is complete; otherwise downloading.
 */
export function aggregateJobs(jobs: Record<string, ProgressPayload>): Aggregate {
  const list = Object.values(jobs);
  let total = 0;
  let downloaded = 0;
  let speed = 0;
  let connections = 0;
  let speedLimit = 0;
  let sha256: string | null | undefined;
  let verified: boolean | null | undefined;
  for (const j of list) {
    total += j.total;
    downloaded += j.downloaded;
    speed += j.speed;
    connections = Math.max(connections, j.connections);
    speedLimit = Math.max(speedLimit, j.speedLimit ?? 0);
    if (j.sha256) sha256 = j.sha256;
    if (j.verified !== undefined && j.verified !== null) verified = j.verified;
  }
  const statuses = list.map((j) => j.status);
  let status = "downloading";
  if (statuses.includes("error")) status = "error";
  else if (statuses.includes("canceled")) status = "canceled";
  else if (statuses.includes("paused")) status = "paused";
  else if (statuses.length > 0 && statuses.every((s) => s === "complete")) status = "complete";
  return {
    total,
    downloaded,
    speed: status === "complete" ? 0 : speed,
    progress: total > 0 ? Math.min(100, (downloaded / total) * 100) : 0,
    eta: speed > 1 && total > downloaded ? (total - downloaded) / speed : -1,
    connections,
    speedLimit,
    status,
    sha256,
    verified,
  };
}

function put(state: CardsState, card: DownloadCard): CardsState {
  return {
    order: state.order.includes(card.id) ? state.order : [...state.order, card.id],
    cards: { ...state.cards, [card.id]: card },
  };
}

function onProgress(state: CardsState, p: ProgressPayload): CardsState {
  const gid = groupId(p.id);
  const existing = state.cards[gid];
  const isYt = streamLabel(p.id) !== null || existing?.isYoutube === true;
  const jobs = { ...(existing?.jobs ?? {}), [p.id]: p };
  const agg = aggregateJobs(jobs);
  return put(state, {
    ...(existing ?? blankCard(gid, p.fileName, agg.status)),
    // Plain files: the engine's name is authoritative (it may come from
    // Content-Disposition after probing). YouTube: set by youtube-status.
    fileName: isYt ? existing?.fileName ?? p.fileName : p.fileName,
    status: existing?.youtubeStatus === "muxing" ? "muxing" : agg.status,
    total: agg.total,
    downloaded: agg.downloaded,
    speed: agg.speed,
    progress: agg.progress,
    eta: agg.eta,
    connections: agg.connections,
    isYoutube: isYt,
    speedLimit: agg.speedLimit,
    sha256: agg.sha256 ?? existing?.sha256,
    verified: agg.verified ?? existing?.verified,
    jobs,
    youtubeStatus: existing?.youtubeStatus,
    error: p.error ?? existing?.error,
    queuePosition: undefined,
  });
}

function onYoutube(state: CardsState, s: YoutubeStatus): CardsState {
  const gid = groupId(s.id);
  const base = state.cards[gid] ?? blankCard(gid, s.fileName, s.status);
  return put(state, {
    ...base,
    isYoutube: true,
    fileName: s.fileName || base.fileName,
    youtubeStatus: s.status,
    status: s.status,
    progress: s.status === "complete" ? 100 : base.progress,
    error: s.error ?? base.error,
  });
}

const QUEUE_STATUSES = new Set(["queued", "scheduled", "held", "starting", "canceled"]);
const WAITING = new Set(["queued", "scheduled", "held"]);

function onQueue(state: CardsState, s: QueueStatus): CardsState {
  const gid = groupId(s.id);
  const existing = state.cards[gid] ?? blankCard(gid, s.fileName, "queued");
  const waiting = WAITING.has(s.status);
  return put(state, {
    ...existing,
    fileName: existing.fileName || s.fileName,
    status: QUEUE_STATUSES.has(s.status) ? s.status : existing.status,
    queuePosition: waiting ? s.position : undefined,
    startAt: s.status === "scheduled" ? s.startAt ?? existing.startAt : null,
    priority: waiting ? s.priority : existing.priority,
    // A (re)start clears the previous run's error.
    error: s.status === "starting" ? null : existing.error,
  });
}

export function cardsReducer(state: CardsState, action: CardsAction): CardsState {
  switch (action.type) {
    case "progress":
      return onProgress(state, action.payload);
    case "youtube":
      return onYoutube(state, action.payload);
    case "queue":
      return onQueue(state, action.payload);
    case "removed": {
      const gone = new Set(action.ids);
      const cards = { ...state.cards };
      for (const id of gone) delete cards[id];
      return { order: state.order.filter((id) => !gone.has(id)), cards };
    }
    case "history": {
      // Persisted downloads go first; cards an event already created win.
      const fresh = action.records.filter((r) => !state.cards[r.id]);
      const cards = { ...state.cards };
      for (const r of fresh) cards[r.id] = cardFromRecord(r);
      return { order: [...fresh.map((r) => r.id), ...state.order], cards };
    }
    case "seed":
      return state.cards[action.card.id] ? state : put(state, action.card);
  }
}

/**
 * Cards in display order. Waiting cards keep the slots they occupy, but are
 * re-ordered among those slots by their backend queue position — so moving an
 * entry up/down (or changing its priority) is visible without anything else
 * jumping around.
 */
export function displayOrder(state: CardsState): DownloadCard[] {
  const cards = state.order.map((id) => state.cards[id]).filter((c): c is DownloadCard => !!c);
  const isWaiting = (c: DownloadCard) => WAITING.has(c.status) && (c.queuePosition ?? 0) > 0;
  const waiting = cards
    .filter(isWaiting)
    .sort((a, b) => (a.queuePosition ?? 0) - (b.queuePosition ?? 0));
  let w = 0;
  return cards.map((c) => (isWaiting(c) ? waiting[w++] : c));
}

/** Total speed / active count shown in the top bar. */
export function globalStats(cards: DownloadCard[]): { speed: number; active: number } {
  let speed = 0;
  let active = 0;
  for (const c of cards) {
    if (c.status === "downloading" || c.status === "muxing") {
      speed += c.speed;
      active++;
    }
  }
  return { speed, active };
}
