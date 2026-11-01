import { describe, expect, it } from "vitest";
import type { BridgeInfo, PendingRequest } from "../types";
import {
  addPending,
  bridgeStatusLine,
  hostOf,
  mergePending,
  parseExtensionIds,
  popupKeyAction,
  removePending,
  requestType,
  SOURCE_LABEL,
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
  shippedExtensionIds: [],
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

describe("download request popup", () => {
  it("names the type of request", () => {
    expect(requestType(req("a"))).toBe("File");
    expect(requestType({ ...req("a"), mediaType: "application/zip" })).toBe("application/zip");
    expect(requestType({ ...req("a"), kind: "stream" })).toBe("Video");
    expect(requestType({ ...req("a"), kind: "stream", quality: "best" })).toBe("Video");
    expect(requestType({ ...req("a"), kind: "stream", quality: "720" })).toBe("Video (720)");
  });

  it("maps Enter to start and Escape to reject, nothing else", () => {
    expect(popupKeyAction("Enter")).toBe("start");
    expect(popupKeyAction("Escape")).toBe("reject");
    expect(popupKeyAction("a")).toBeNull();
    expect(popupKeyAction(" ")).toBeNull();
  });

  it("shows the site and why the browser sent it", () => {
    expect(hostOf("https://cdn.example.com:8443/a.zip?x=1")).toBe("cdn.example.com:8443");
    expect(hostOf("not a url")).toBe("not a url");
    expect(SOURCE_LABEL["user-action"]).toBe("");
    expect(SOURCE_LABEL.takeover).toContain("browser");
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
