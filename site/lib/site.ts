import pkg from "../../package.json";

// The app's version, read from the desktop app's package.json so the site never announces a stale one.
export const version: string = pkg.version;

export const url = "https://nuzky.app";
export const description =
  "A free video editor for TikTok, Reels, Shorts and YouTube. AI writes the captions in your language and cuts the ums, pauses and retakes. No subscription, no watermark.";

export const repo = "https://github.com/tomasmach/nuzky";
export const releaseNotes = `${repo}/releases`;
export const buildFromSource = `${repo}/blob/main/docs/BUILDING.md`;
export const license = `${repo}/blob/main/LICENSE`;

export const platforms = [
  { id: "macos", name: "macOS", detail: "Apple Silicon · .dmg" },
  { id: "windows", name: "Windows", detail: "x64 · Installer" },
  { id: "linux", name: "Linux", detail: "AppImage · .deb" },
] as const;
