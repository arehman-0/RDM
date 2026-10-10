import { describe, expect, it } from "vitest";
import {
  guessName,
  isPrerequisiteError,
  parseHeaders,
  parseMirrors,
  scheduleToEpoch,
  validateUrl,
} from "./input";

describe("validateUrl", () => {
  it("accepts http(s) links", () => {
    expect(validateUrl("https://example.com/file.iso")).toBeNull();
    expect(validateUrl("  http://127.0.0.1:8765/big.bin  ")).toBeNull();
    expect(validateUrl("https://www.youtube.com/watch?v=abc")).toBeNull();
  });

  it("explains what is wrong", () => {
    expect(validateUrl("")).toMatch(/paste a link/i);
    expect(validateUrl("   ")).toMatch(/paste a link/i);
    expect(validateUrl("example.com/file.zip")).toMatch(/add http/i);
    expect(validateUrl("not a url")).toMatch(/spaces/i);
    expect(validateUrl("hello")).toMatch(/doesn't look like a link/i);
    expect(validateUrl("ftp://host/file")).toMatch(/only http/i);
    expect(validateUrl("file:///C:/x.txt")).toMatch(/only http/i);
    expect(validateUrl("javascript:alert(1)")).toMatch(/only http/i);
  });
});

describe("parseHeaders / parseMirrors", () => {
  it("parses Key: Value lines, ignoring junk", () => {
    expect(parseHeaders("Authorization: Bearer a:b\n\nno colon\n: empty key\nX-A:1")).toEqual({
      Authorization: "Bearer a:b",
      "X-A": "1",
    });
    expect(parseHeaders("  \n")).toBeNull();
  });

  it("parses one mirror per line", () => {
    expect(parseMirrors(" https://a/x \n\nhttps://b/x\n")).toEqual(["https://a/x", "https://b/x"]);
    expect(parseMirrors("")).toBeNull();
  });
});

describe("scheduleToEpoch", () => {
  it("empty means start now; local time converts to epoch seconds", () => {
    expect(scheduleToEpoch("")).toBeNull();
    expect(scheduleToEpoch("2026-10-08T16:28")).toBe(
      Math.floor(new Date(2026, 9, 8, 16, 28).getTime() / 1000)
    );
    expect(Number.isNaN(scheduleToEpoch("garbage"))).toBe(true);
  });
});

describe("guessName", () => {
  it("uses the last path segment", () => {
    expect(guessName("https://h/a/b/file.iso?x=1#y")).toBe("file.iso");
    expect(guessName("https://h/")).toBe("download.bin");
  });
});

describe("isPrerequisiteError", () => {
  it("recognises the backend's preflight message", () => {
    expect(
      isPrerequisiteError(
        "Missing required tool(s) for video downloads: yt-dlp and ffmpeg. Install with: winget install yt-dlp.yt-dlp"
      )
    ).toBe(true);
    expect(isPrerequisiteError("server returned HTTP 404 Not Found")).toBe(false);
  });
});
