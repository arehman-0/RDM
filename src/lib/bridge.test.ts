import { describe, expect, it } from "vitest";
import type { BridgeInfo, PendingRequest } from "../types";
import {
  addPending,
  bridgeStatusLine,
  mergePending,
  parseExtensionIds,
  removePending,
} from "./bridge";

const req = (id: string): PendingRequest => ({
  id,
  status: "pending-confirm",
  url: `https://example.com/${id}`,
  pageUrl: null,
  fileName: `${id}.bin`,
  kind: "file",
  mediaType: null,
  size: null,
  headerNames: [],
  hasCookies: false,
  receivedAt: 1,
  source: "user-action",
  quality: null,
});

const info = (over: Partial<BridgeInfo>): BridgeInfo => ({
  settings: { enabled: true, allowedExtensionIds: [], askBeforeStart: true, confirmTakeover: false },
  listening: false,
  port: null,
  address: null,
  error: null,
  token: "t",
  defaultPort: 38917,
  protocol: 1,
  ...over,
});

describe("pending requests", () => {
  it("adds once, removes by id, merges backlog with live", () => {
    let list = addPending([], req("a"));
    list = addPending(list, req("a"));
    expect(list.map((r) => r.id)).toEqual(["a"]);
    expect(removePending(list, "a")).toEqual([]);
    expect(mergePending([req("a"), req("b")], [req("b"), req("c")]).map((r) => r.id)).toEqual([
      "a",
      "b",
      "c",
    ]);
  });
});

describe("parseExtensionIds", () => {
  it("splits on newlines, commas and spaces", () => {
    expect(parseExtensionIds(" aaa\nbbb, ccc  \n\n")).toEqual(["aaa", "bbb", "ccc"]);
    expect(parseExtensionIds("")).toEqual([]);
  });
});

describe("bridgeStatusLine", () => {
  it("describes each state", () => {
    expect(bridgeStatusLine(null)).toBe("Loading…");
    expect(bridgeStatusLine(info({ listening: true, port: 38917, address: "127.0.0.1:38917" }))).toBe(
      "Listening on 127.0.0.1:38917"
    );
    expect(bridgeStatusLine(info({ listening: true, port: 38918, address: "127.0.0.1:38918" }))).toBe(
      "Listening on 127.0.0.1:38918 (port 38917 was busy)"
    );
    expect(bridgeStatusLine(info({ error: "ports busy" }))).toBe("Not listening: ports busy");
    expect(
      bridgeStatusLine(info({ settings: { enabled: false, allowedExtensionIds: [], askBeforeStart: true, confirmTakeover: false } }))
    ).toBe("Off");
  });
});
