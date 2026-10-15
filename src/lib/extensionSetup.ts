// The "Install the browser extension" step: which buttons to offer and the
// manual (developer mode) instructions for the browser the user picked.

import type { BrowserId, ExtensionSetupInfo, InstalledBrowser } from "../types";

export interface StoreButton {
  store: "chrome" | "edge";
  label: string;
  /** Not published there yet: shown disabled. */
  available: boolean;
}

export function storeButtons(info: ExtensionSetupInfo): StoreButton[] {
  return [
    { store: "chrome", label: "Open Chrome Web Store page", available: info.chromeWebStoreUrl !== null },
    { store: "edge", label: "Open Edge Add-ons page", available: info.edgeAddonsUrl !== null },
  ];
}

/** The page to type in the address bar for each browser. */
export function extensionsPage(id: BrowserId): string {
  return `${id}://extensions`;
}

/** The exact steps for "Load unpacked" in `browser`. */
export function manualSteps(browser: InstalledBrowser | null, folder: string | null): string[] {
  const id: BrowserId = browser?.id ?? "chrome";
  const name = browser?.name ?? "your browser";
  const devMode =
    id === "edge"
      ? "Turn on “Developer mode” (left side of the page)."
      : "Turn on “Developer mode” (top right of the page).";
  return [
    `In ${name}, open ${extensionsPage(id)} (the button below does this).`,
    devMode,
    "Click “Load unpacked”.",
    `Choose this folder: ${folder ?? "the “extension” folder inside the IDM Replica install folder"}`,
  ];
}

/** The browser to tailor the instructions to: Chrome, else Edge, else Brave. */
export function preferredBrowser(browsers: InstalledBrowser[]): InstalledBrowser | null {
  for (const id of ["chrome", "edge", "brave"] as const) {
    const b = browsers.find((x) => x.id === id);
    if (b) return b;
  }
  return null;
}
