import { describe, expect, it } from "vitest";
import { manualSteps, preferredBrowser, storeButtons } from "./extensionSetup";
import type { ExtensionSetupInfo, InstalledBrowser } from "../types";

const chrome: InstalledBrowser = { id: "chrome", name: "Google Chrome", path: "C:\\c\\chrome.exe" };
const edge: InstalledBrowser = { id: "edge", name: "Microsoft Edge", path: "C:\\e\\msedge.exe" };
const brave: InstalledBrowser = { id: "brave", name: "Brave", path: "C:\\b\\brave.exe" };

describe("extension setup", () => {
  it("disables store buttons until the listing exists", () => {
    const info: ExtensionSetupInfo = {
      browsers: [],
      chromeWebStoreUrl: null,
      edgeAddonsUrl: "https://microsoftedge.microsoft.com/addons/detail/x",
      unpackedDir: null,
    };
    expect(storeButtons(info).map((b) => [b.store, b.available])).toEqual([
      ["chrome", false],
      ["edge", true],
    ]);
  });

  it("tailors the manual steps to the browser", () => {
    const steps = manualSteps(edge, "C:\\IDM\\resources\\extension");
    expect(steps).toHaveLength(4);
    expect(steps[0]).toContain("edge://extensions");
    expect(steps[1]).toContain("left side");
    expect(steps[3]).toContain("C:\\IDM\\resources\\extension");
    expect(manualSteps(brave, null)[0]).toContain("brave://extensions");
    expect(manualSteps(null, null)[0]).toContain("chrome://extensions");
  });

  it("prefers Chrome, then Edge, then Brave", () => {
    expect(preferredBrowser([brave, edge, chrome])).toBe(chrome);
    expect(preferredBrowser([brave, edge])).toBe(edge);
    expect(preferredBrowser([brave])).toBe(brave);
    expect(preferredBrowser([])).toBeNull();
  });
});
