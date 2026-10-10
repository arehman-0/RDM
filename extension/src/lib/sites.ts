// Known stream sites: their players use DASH/HLS over blob: URLs with signed,
// short-lived segment URLs, so we never sniff them. Instead the page URL is
// sent with kind "stream" and the app's yt-dlp pipeline resolves it.
// This is the ONE place to edit the list; keep the video-page rules in sync
// with the app (src-tauri/src/youtube.rs → is_video_page).

export interface KnownSite {
  name: string;
  /** Page domains (a subdomain matches too, e.g. m.youtube.com). */
  domains: string[];
  /** Media CDNs whose requests are ignored by the network sniffer. */
  cdnDomains: string[];
  /** Which pages hold one downloadable video (lower-cased host + path). Absent = every page. */
  isVideo?: (host: string, path: string) => boolean;
}

const starts = (path: string, prefixes: string[]) => prefixes.some((p) => path.startsWith(p));

export const KNOWN_SITES: KnownSite[] = [
  {
    name: "YouTube",
    domains: ["youtube.com", "youtu.be", "youtube-nocookie.com"],
    cdnDomains: ["googlevideo.com", "ytimg.com"],
    isVideo: (host, path) =>
      /^\/(watch|shorts\/|embed\/|live\/)/.test(path) || (hostMatches(host, "youtu.be") && /^\/[\w-]{11}$/.test(path)),
  },
  { name: "Vimeo", domains: ["vimeo.com"], cdnDomains: ["vimeocdn.com"] },
  { name: "Twitter / X", domains: ["twitter.com", "x.com"], cdnDomains: ["twimg.com"] },
  {
    name: "Facebook",
    domains: ["facebook.com", "fb.watch"],
    cdnDomains: ["fbcdn.net"],
    isVideo: (host, path) =>
      hostMatches(host, "fb.watch")
        ? path.length > 1
        : starts(path, ["/watch", "/reel/", "/reels/", "/share/v/", "/share/r/", "/videos/"]) || path.includes("/videos/"),
  },
  {
    name: "Instagram",
    domains: ["instagram.com"],
    cdnDomains: ["cdninstagram.com", "fbcdn.net"],
    isVideo: (_h, path) => starts(path, ["/p/", "/reel/", "/reels/", "/tv/"]),
  },
  {
    name: "TikTok",
    domains: ["tiktok.com"],
    cdnDomains: ["tiktokcdn.com", "tiktokcdn-us.com", "tiktokv.com"],
    isVideo: (host, path) =>
      hostMatches(host, "vm.tiktok.com") || hostMatches(host, "vt.tiktok.com")
        ? path.length > 1
        : path.includes("/video/") || path.startsWith("/t/"),
  },
  {
    name: "Reddit",
    domains: ["reddit.com", "redd.it"],
    cdnDomains: ["redd.it", "redditmedia.com"],
    isVideo: (host, path) => (hostMatches(host, "redd.it") ? path.length > 1 : path.includes("/comments/")),
  },
  {
    name: "Twitch",
    domains: ["twitch.tv"],
    cdnDomains: ["ttvnw.net", "jtvnw.net"],
    // VODs and clips; a live channel would never finish downloading.
    isVideo: (host, path) =>
      hostMatches(host, "clips.twitch.tv") ? path.length > 1 : path.startsWith("/videos/") || path.includes("/clip/"),
  },
  {
    name: "Dailymotion",
    domains: ["dailymotion.com", "dai.ly"],
    cdnDomains: ["dmcdn.net"],
    isVideo: (host, path) => (hostMatches(host, "dai.ly") ? path.length > 1 : path.startsWith("/video/")),
  },
];

/** `host` is `domain` or a subdomain of it — never a substring match. */
export function hostMatches(host: string, domain: string): boolean {
  const h = host.toLowerCase().replace(/\.$/, "");
  return h === domain || h.endsWith(`.${domain}`);
}

function hostOf(url: string): string | null {
  try {
    const u = new URL(url);
    return u.protocol === "http:" || u.protocol === "https:" ? u.hostname : null;
  } catch {
    return null;
  }
}

/** The known site a page URL belongs to, if any. */
export function knownSiteFor(pageUrl: string): KnownSite | null {
  const host = hostOf(pageUrl);
  if (!host) return null;
  return KNOWN_SITES.find((s) => s.domains.some((d) => hostMatches(host, d))) ?? null;
}

/** A request to a known site's media CDN (never listed: signed fragments). */
export function isKnownSiteCdn(url: string): boolean {
  const host = hostOf(url);
  if (!host) return false;
  return KNOWN_SITES.some((s) => s.cdnDomains.some((d) => hostMatches(host, d)));
}

/** A known-site page that shows a video (e.g. YouTube watch / Shorts, not the home page). */
export function isKnownVideoPage(pageUrl: string): boolean {
  const site = knownSiteFor(pageUrl);
  if (!site) return false;
  if (!site.isVideo) return true;
  try {
    const u = new URL(pageUrl);
    return site.isVideo(u.hostname.toLowerCase(), u.pathname.toLowerCase());
  } catch {
    return false;
  }
}
