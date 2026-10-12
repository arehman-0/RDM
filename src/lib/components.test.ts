import { describe, expect, it } from "vitest";
import type { ComponentProgress, ComponentsInfo } from "../types";
import { applyProgress, describeOutcome, needsSetup, percent, phaseText, sourceText } from "./components";

const info = (over: Partial<ComponentsInfo>): ComponentsInfo => ({
  components: [],
  missing: [],
  missingBytes: 0,
  settingUp: false,
  autoUpdate: true,
  error: null,
  ...over,
});
const prog = (over: Partial<ComponentProgress>): ComponentProgress => ({
  id: "yt-dlp",
  name: "yt-dlp",
  phase: "downloading",
  downloaded: 0,
  total: 100,
  error: null,
  ...over,
});

describe("setup screen", () => {
  it("shows only while something is missing or installing, unless skipped", () => {
    expect(needsSetup(null, false)).toBe(false);
    expect(needsSetup(info({}), false)).toBe(false);
    expect(needsSetup(info({ missing: ["ffmpeg"] }), false)).toBe(true);
    expect(needsSetup(info({ settingUp: true }), false)).toBe(true);
    expect(needsSetup(info({ missing: ["ffmpeg"] }), true)).toBe(false);
  });
  it("computes bars and phase labels", () => {
    expect(percent(undefined)).toBe(0);
    expect(percent(prog({ downloaded: 25 }))).toBe(25);
    expect(percent(prog({ downloaded: 500 }))).toBe(100);
    expect(percent(prog({ total: 0 }))).toBe(0);
    expect(percent(prog({ phase: "verifying" }))).toBe(100);
    expect(phaseText(undefined, false)).toBe("Waiting…");
    expect(phaseText(undefined, true)).toBe("Ready");
    expect(phaseText(prog({ phase: "error" }), false)).toBe("Failed");
    expect(phaseText(prog({ phase: "downloading" }), false)).toBe("Downloading…");
  });
  it("keeps the latest event per component", () => {
    let m = applyProgress({}, prog({ downloaded: 1 }));
    m = applyProgress(m, prog({ id: "ffmpeg", downloaded: 5 }));
    m = applyProgress(m, prog({ downloaded: 9 }));
    expect(m["yt-dlp"].downloaded).toBe(9);
    expect(Object.keys(m).sort()).toEqual(["ffmpeg", "yt-dlp"]);
  });
});

describe("texts", () => {
  it("describes update outcomes and sources", () => {
    expect(describeOutcome(null)).toBe("");
    expect(describeOutcome({ result: "upToDate", version: "2026.08.19" })).toContain("latest");
    expect(describeOutcome({ result: "updated", from: "a", to: "b" })).toBe("Updated yt-dlp from a to b.");
    expect(describeOutcome({ result: "rolledBack", reason: "x" })).toContain("previous version was kept");
    expect(describeOutcome({ result: "skipped", reason: "checked recently" })).toBe("Not updated: checked recently.");
    expect(sourceText(null)).toBe("not installed");
    expect(sourceText("path")).toContain("PATH");
  });
});
