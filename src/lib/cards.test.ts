import { describe, expect, it } from "vitest";
import type { JobRecord, ProgressPayload, QueueStatus } from "../types";
import {
  aggregateJobs,
  blankCard,
  cardsReducer,
  displayOrder,
  emptyCards,
  globalStats,
  groupId,
  streamLabel,
  type CardsAction,
  type CardsState,
} from "./cards";
import { cardActions, statusText } from "./status";

function progress(id: string, over: Partial<ProgressPayload> = {}): ProgressPayload {
  return {
    id,
    fileName: `${id}.bin`,
    url: "https://h/x",
    total: 1000,
    downloaded: 0,
    speed: 0,
    eta: -1,
    progress: 0,
    status: "downloading",
    connections: 4,
    activeThreads: 4,
    speedLimit: 0,
    threads: [],
    ...over,
  };
}

function queue(id: string, over: Partial<QueueStatus> = {}): QueueStatus {
  return {
    id,
    fileName: `${id}.bin`,
    status: "queued",
    position: 1,
    priority: "normal",
    held: false,
    ...over,
  };
}

const run = (...actions: CardsAction[]): CardsState => actions.reduce(cardsReducer, emptyCards);

describe("ids", () => {
  it("groups YouTube sub-jobs under their parent", () => {
    expect(groupId("abc::video")).toBe("abc");
    expect(groupId("abc")).toBe("abc");
    expect(streamLabel("abc::audio")).toBe("audio");
    expect(streamLabel("abc")).toBeNull();
  });
});

describe("aggregation reducer — plain files", () => {
  it("creates a card from the first progress event and tracks it", () => {
    const s = run(
      { type: "progress", payload: progress("f", { downloaded: 250, speed: 100 }) },
      { type: "progress", payload: progress("f", { downloaded: 500, speed: 250, fileName: "real.iso" }) }
    );
    expect(s.order).toEqual(["f"]);
    const c = s.cards.f;
    expect(c).toMatchObject({
      fileName: "real.iso", // engine's (Content-Disposition) name wins for files
      status: "downloading",
      total: 1000,
      downloaded: 500,
      progress: 50,
      speed: 250,
      eta: 2,
      isYoutube: false,
    });
  });

  it("zeroes speed on completion and keeps checksum results", () => {
    const s = run({
      type: "progress",
      payload: progress("f", { status: "complete", downloaded: 1000, speed: 999, sha256: "ab", verified: true }),
    });
    expect(s.cards.f).toMatchObject({ status: "complete", speed: 0, progress: 100, sha256: "ab", verified: true });
  });

  it("keeps the last error until a newer one", () => {
    const s = run(
      { type: "progress", payload: progress("f", { status: "error", error: "HTTP 404" }) },
      { type: "progress", payload: progress("f", { status: "error" }) }
    );
    expect(s.cards.f.error).toBe("HTTP 404");
  });
});

describe("YouTube video/audio sub-stream merging", () => {
  const video = (over: Partial<ProgressPayload>) => progress("y::video", { total: 800, ...over });
  const audio = (over: Partial<ProgressPayload>) => progress("y::audio", { total: 200, ...over });

  it("sums both streams into one card", () => {
    const s = run(
      { type: "youtube", payload: { id: "y", fileName: "Clip.mp4", status: "downloading" } },
      { type: "progress", payload: video({ downloaded: 400, speed: 100, connections: 8, speedLimit: 500 }) },
      { type: "progress", payload: audio({ downloaded: 200, speed: 50, connections: 4, speedLimit: 500 }) }
    );
    expect(s.order).toEqual(["y"]);
    expect(s.cards.y).toMatchObject({
      isYoutube: true,
      fileName: "Clip.mp4", // not the temp stream name
      total: 1000,
      downloaded: 600,
      progress: 60,
      speed: 150,
      connections: 8,
      speedLimit: 500, // shared cap, not doubled
    });
    expect(Object.keys(s.cards.y.jobs).sort()).toEqual(["y::audio", "y::video"]);
  });

  it("combines statuses: error > canceled > paused; complete needs both", () => {
    const status = (v: string, a: string) =>
      aggregateJobs({
        "y::video": video({ status: v }),
        "y::audio": audio({ status: a }),
      }).status;
    expect(status("complete", "downloading")).toBe("downloading");
    expect(status("complete", "complete")).toBe("complete");
    expect(status("paused", "downloading")).toBe("paused");
    expect(status("canceled", "paused")).toBe("canceled");
    expect(status("error", "canceled")).toBe("error");
    expect(aggregateJobs({}).status).toBe("downloading");
  });

  it("stays 'muxing' while sub-job progress still trickles in", () => {
    const s = run(
      { type: "progress", payload: video({ status: "complete", downloaded: 800 }) },
      { type: "youtube", payload: { id: "y", fileName: "Clip.mp4", status: "muxing" } },
      { type: "progress", payload: audio({ status: "complete", downloaded: 200 }) }
    );
    expect(s.cards.y.status).toBe("muxing");
    const done = cardsReducer(s, {
      type: "youtube",
      payload: { id: "y", fileName: "Clip.mp4", status: "complete" },
    });
    expect(done.cards.y).toMatchObject({ status: "complete", progress: 100 });
  });
});

describe("queue-status handling", () => {
  it("tracks position, priority, schedule and holds", () => {
    let s = run({ type: "queue", payload: queue("q", { position: 3, priority: "high" }) });
    expect(s.cards.q).toMatchObject({ status: "queued", queuePosition: 3, priority: "high" });
    expect(statusText(s.cards.q)).toBe("Queued · #3");

    s = cardsReducer(s, { type: "queue", payload: queue("q", { status: "held", held: true, position: 2 }) });
    expect(s.cards.q).toMatchObject({ status: "held", queuePosition: 2 });
    expect(statusText(s.cards.q)).toBe("Paused in queue · #2");
    expect(cardActions(s.cards.q)).toEqual(["reorder", "resume", "cancel"]);

    s = cardsReducer(s, { type: "queue", payload: queue("q", { status: "scheduled", startAt: 1_900_000_000 }) });
    expect(s.cards.q).toMatchObject({ status: "scheduled", startAt: 1_900_000_000 });
    expect(statusText(s.cards.q)).toMatch(/^Scheduled for /);
  });

  it("'starting' leaves the queue and clears the previous error", () => {
    const s = run(
      { type: "progress", payload: progress("r", { status: "error", error: "boom" }) },
      { type: "queue", payload: queue("r", { status: "starting", position: 0 }) }
    );
    expect(s.cards.r).toMatchObject({ status: "starting", error: null, queuePosition: undefined, startAt: null });
  });

  it("'canceled' sticks; unknown statuses keep the current one", () => {
    let s = run({ type: "queue", payload: queue("c", { status: "canceled", position: 0 }) });
    expect(s.cards.c.status).toBe("canceled");
    s = cardsReducer(s, { type: "queue", payload: queue("c", { status: "mystery" }) });
    expect(s.cards.c.status).toBe("canceled");
  });

  it("reorders waiting cards among their own slots by queue position", () => {
    let s = run(
      { type: "progress", payload: progress("busy") },
      { type: "queue", payload: queue("a", { position: 1 }) },
      { type: "progress", payload: progress("other") },
      { type: "queue", payload: queue("b", { position: 2 }) }
    );
    expect(displayOrder(s).map((c) => c.id)).toEqual(["busy", "a", "other", "b"]);
    // Backend moved b above a.
    s = cardsReducer(s, { type: "queue", payload: queue("b", { position: 1 }) });
    s = cardsReducer(s, { type: "queue", payload: queue("a", { position: 2 }) });
    expect(displayOrder(s).map((c) => c.id)).toEqual(["busy", "b", "other", "a"]);
  });
});

describe("history, removal and seeding", () => {
  const record = (id: string, over: Partial<JobRecord> = {}): JobRecord => ({
    id,
    kind: "file",
    url: "https://h/x",
    mirrors: [],
    headerNames: [],
    dir: "C:/dl",
    dest: null,
    fileName: `${id}.iso`,
    status: "interrupted",
    total: 400,
    downloaded: 100,
    connections: 8,
    speedLimit: 0,
    verify: false,
    expectedChecksum: null,
    startAt: null,
    priority: "normal",
    held: false,
    createdAt: 0,
    updatedAt: 0,
    error: null,
    ...over,
  });

  it("restores persisted downloads first; live cards win", () => {
    const s = run(
      { type: "progress", payload: progress("live", { downloaded: 10 }) },
      {
        type: "history",
        records: [record("old"), record("live", { downloaded: 999 }), record("h", { status: "queued", held: true })],
      }
    );
    expect(s.order).toEqual(["old", "h", "live"]);
    expect(s.cards.old).toMatchObject({ status: "interrupted", progress: 25 });
    expect(s.cards.live.downloaded).toBe(10);
    expect(s.cards.h.status).toBe("held");
    expect(cardActions(s.cards.old)).toEqual(["retry", "reveal", "remove"]);
  });

  it("removes cards and their order entries", () => {
    const s = cardsReducer(run({ type: "history", records: [record("a"), record("b")] }), {
      type: "removed",
      ids: ["a", "zzz"],
    });
    expect(s.order).toEqual(["b"]);
    expect(s.cards.a).toBeUndefined();
  });

  it("seeding never overwrites a card an event already created", () => {
    const s = run(
      { type: "progress", payload: progress("x", { downloaded: 7 }) },
      { type: "seed", card: blankCard("x", "x.bin", "starting") }
    );
    expect(s.cards.x.downloaded).toBe(7);
  });
});

describe("globalStats", () => {
  it("sums speed of active cards only", () => {
    const s = run(
      { type: "progress", payload: progress("a", { speed: 100 }) },
      { type: "progress", payload: progress("b", { speed: 50, status: "paused" }) },
      { type: "progress", payload: progress("c", { speed: 25 }) }
    );
    expect(globalStats(displayOrder(s))).toEqual({ speed: 125, active: 2 });
  });
});
