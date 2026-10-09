import pkg from "../../package.json";

// The app's version, read from the desktop app's package.json so the site never announces a stale one.
export const version: string = pkg.version;

export const url = "https://nuzky.app";
export const description =
  "A free, open source video editor for Reels, TikTok and Shorts on Mac, Windows and Linux. Auto captions on your computer, AI that edits for you, no watermark.";

export const repo = "https://github.com/tomasmach/nuzky";
export const releaseNotes = `${repo}/releases`;
export const buildFromSource = `${repo}/blob/main/docs/BUILDING.md`;
export const license = `${repo}/blob/main/LICENSE`;

export const platforms = [
  { id: "macos", name: "macOS", detail: "Apple Silicon · .dmg" },
  { id: "windows", name: "Windows", detail: "x64 · Installer" },
  { id: "linux", name: "Linux", detail: "AppImage · .deb" },
] as const;
