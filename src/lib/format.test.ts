import { describe, expect, it } from "vitest";
import { fmtBytes, fmtClock, fmtEta, fmtSpeed } from "./format";

describe("fmtBytes", () => {
  it.each([
    [0, "0 B"],
    [1, "1.0 B"],
    [512, "512.0 B"],
    [1023, "1023.0 B"],
    [1024, "1.00 KB"],
    [1536, "1.50 KB"],
    [10 * 1024, "10.0 KB"],
    [5.5 * 1024 * 1024, "5.50 MB"],
    [24 * 1024 * 1024, "24.0 MB"],
    [3 * 1024 ** 3, "3.00 GB"],
    [2048 * 1024 ** 4, "2048.0 TB"], // largest unit caps
  ])("%d → %s", (n, want) => {
    expect(fmtBytes(n)).toBe(want);
  });

  it("treats junk as zero", () => {
    expect(fmtBytes(-5)).toBe("0 B");
    expect(fmtBytes(Number.NaN)).toBe("0 B");
    expect(fmtBytes(Number.POSITIVE_INFINITY)).toBe("0 B");
  });
});

describe("fmtSpeed", () => {
  it("appends /s", () => {
    expect(fmtSpeed(0)).toBe("0 B/s");
    expect(fmtSpeed(1024 * 1024)).toBe("1.00 MB/s");
    expect(fmtSpeed(995.9 * 1024)).toBe("995.9 KB/s");
  });
});

describe("fmtEta", () => {
  it.each([
    [0, "0s"],
    [59.4, "59s"],
    [59.6, "1m 0s"], // rounds before splitting
    [61, "1m 1s"],
    [3599, "59m 59s"],
    [3600, "1h 0m"],
    [3725, "1h 2m"],
    [90000, "25h 0m"],
  ])("%d s → %s", (s, want) => {
    expect(fmtEta(s)).toBe(want);
  });

  it("unknown is a dash", () => {
    expect(fmtEta(-1)).toBe("—");
    expect(fmtEta(Number.POSITIVE_INFINITY)).toBe("—");
    expect(fmtEta(Number.NaN)).toBe("—");
  });
});

describe("fmtClock", () => {
  it("zero-pads local time", () => {
    expect(fmtClock(new Date(2026, 0, 2, 3, 4, 5).getTime())).toBe("03:04:05");
  });
});
