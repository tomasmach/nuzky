import type { Adjust, AnimationKind, TextStyle, Transform, TransitionKind } from "./types";

export const FORMATS = [
  { label: "9:16", hint: "Reels, TikTok, Shorts", width: 1080, height: 1920 },
  { label: "16:9", hint: "YouTube", width: 1920, height: 1080 },
  { label: "1:1", hint: "Square", width: 1080, height: 1080 },
  { label: "4:5", hint: "Instagram feed", width: 1080, height: 1350 },
];

export function formatLabel(width: number, height: number) {
  return FORMATS.find((f) => f.width === width && f.height === height)?.label ?? `${width}×${height}`;
}

export const DEFAULT_TRANSFORM: Transform = { x: 0, y: 0, scale: 1, rotation: 0, opacity: 1 };
/** Where the engine places generated captions (CAPTION_Y in crates/engine edit.rs). */
export const CAPTION_Y = 0.15;
export const NO_ADJUST: Adjust = { exposure: 0, tint: 0, highlights: 0, shadows: 0, fade: 0, brightness: 0, contrast: 0, saturation: 0, temperature: 0, vignette: 0 };

export const TEXT_PRESETS: { name: string; text: string; style: TextStyle }[] = [
  { name: "Classic", text: "Your text", style: { fontSize: 84, color: "#ffffff", bold: true, strokeWidth: 7, strokeColor: "#000000", background: null } },
  { name: "Title", text: "BIG TITLE", style: { fontSize: 130, color: "#ffffff", bold: true, strokeWidth: 0, strokeColor: "#000000", background: null } },
  { name: "Label", text: "Label", style: { fontSize: 72, color: "#111111", bold: true, strokeWidth: 0, strokeColor: "#000000", background: "#ffffffee" } },
  { name: "Highlight", text: "Highlight", style: { fontSize: 84, color: "#111111", bold: true, strokeWidth: 0, strokeColor: "#000000", background: "#ffd400ee" } },
  { name: "Neon", text: "Neon", style: { fontSize: 96, color: "#9cffd9", bold: true, strokeWidth: 5, strokeColor: "#0b6b4f", background: null } },
  { name: "Plain", text: "Plain text", style: { fontSize: 64, color: "#ffffff", bold: false, strokeWidth: 0, strokeColor: "#000000", background: null } },
];

/** Presets carry no font: the font is picked separately and survives a preset change. */
export const CAPTION_STYLES: { name: string; style: TextStyle }[] = [
  // Talking-head reels: regular weight, thick outline, no box, 1–3 words at a time.
  { name: "Reel", style: { fontSize: 95, color: "#ffffff", bold: false, strokeWidth: 7.5, strokeColor: "#000000", background: null } },
  { name: "Outline", style: { fontSize: 70, color: "#ffffff", bold: true, strokeWidth: 7, strokeColor: "#000000", background: null } },
  { name: "Yellow", style: { fontSize: 74, color: "#ffe14d", bold: true, strokeWidth: 7, strokeColor: "#000000", background: null } },
  { name: "Box", style: { fontSize: 62, color: "#ffffff", bold: true, strokeWidth: 0, strokeColor: "#000000", background: "#000000b3" } },
  { name: "Clean", style: { fontSize: 60, color: "#ffffff", bold: false, strokeWidth: 3, strokeColor: "#00000099", background: null } },
];

/** Same look apart from the font. */
export function sameStyle(a: TextStyle, b: TextStyle) {
  return (
    a.fontSize === b.fontSize &&
    a.color.toLowerCase() === b.color.toLowerCase() &&
    a.bold === b.bold &&
    a.strokeWidth === b.strokeWidth &&
    a.strokeColor.toLowerCase() === b.strokeColor.toLowerCase() &&
    (a.background ?? "").toLowerCase() === (b.background ?? "").toLowerCase()
  );
}

/** Typewriter only makes sense for text; the engine treats it as Fade on media. */
export const ANIMATIONS: { kind: AnimationKind; label: string; textOnly?: boolean }[] = [
  { kind: "fade", label: "Fade" },
  { kind: "zoomIn", label: "Zoom in" },
  { kind: "zoomOut", label: "Zoom out" },
  { kind: "slideUp", label: "Slide up" },
  { kind: "slideDown", label: "Slide down" },
  { kind: "slideLeft", label: "Slide left" },
  { kind: "slideRight", label: "Slide right" },
  { kind: "pop", label: "Pop" },
  { kind: "typewriter", label: "Typewriter", textOnly: true },
];

export const TRANSITIONS: { kind: TransitionKind; label: string }[] = [
  { kind: "dissolve", label: "Dissolve" },
  { kind: "fadeBlack", label: "Fade to black" },
  { kind: "fadeWhite", label: "Flash white" },
  { kind: "slideLeft", label: "Slide left" },
  { kind: "slideUp", label: "Slide up" },
  { kind: "zoomIn", label: "Zoom in" },
  { kind: "wipeLeft", label: "Wipe left" },
  { kind: "blur", label: "Blur" },
];

export const DEFAULT_TRANSITION_US = 500_000;
export const MAX_TRANSITION_US = 2_000_000;

export const FILTERS: { id: string; label: string; adjust: Adjust }[] = [
  { id: "none", label: "None", adjust: NO_ADJUST },
  { id: "vivid", label: "Vivid", adjust: { ...NO_ADJUST, brightness: 0.03, contrast: 0.15, saturation: 0.35 } },
  { id: "warm", label: "Warm", adjust: { ...NO_ADJUST, saturation: 0.1, temperature: 0.35 } },
  { id: "cool", label: "Cool", adjust: { ...NO_ADJUST, brightness: 0.02, temperature: -0.35 } },
  { id: "mono", label: "Mono", adjust: { ...NO_ADJUST, contrast: 0.15, saturation: -1 } },
  { id: "fade", label: "Fade", adjust: { ...NO_ADJUST, fade: 0.5, contrast: -0.1, saturation: -0.2 } },
  { id: "moody", label: "Moody", adjust: { ...NO_ADJUST, shadows: -0.25, brightness: -0.06, contrast: 0.2, saturation: -0.25, temperature: -0.1, vignette: 0.45 } },
  { id: "punch", label: "Punch", adjust: { ...NO_ADJUST, contrast: 0.35, saturation: 0.25, vignette: 0.25 } },
];

export function sameAdjust(a: Adjust, b: Adjust) {
  return (Object.keys(NO_ADJUST) as (keyof Adjust)[]).every((k) => Math.abs(a[k] - b[k]) < 0.005);
}

/** Rough CSS approximation of an Adjust, for preset tiles only. The engine does the real grading. */
export function adjustCss(a: Adjust): { filter: string; tint: string | null; vignette: number } {
  const light = (1 + a.brightness) * 2 ** a.exposure + a.highlights * 0.1 + a.shadows * 0.1;
  const contrast = (1 + a.contrast + a.highlights * 0.1 - a.shadows * 0.1) * (1 - a.fade * 0.3);
  const filter = `brightness(${Math.max(0, light)}) contrast(${Math.max(0, contrast)}) saturate(${1 + a.saturation})`;
  const temperature = Math.abs(a.temperature);
  const magenta = Math.abs(a.tint);
  const weight = temperature + magenta;
  const warm = a.temperature > 0 ? [255, 150, 40] : [40, 130, 255];
  const cast = a.tint > 0 ? [255, 40, 255] : [40, 255, 40];
  const rgb = warm.map((v, i) => Math.round((v * temperature + cast[i] * magenta) / (weight || 1)));
  const tint = weight === 0 ? null : `rgba(${rgb.join(",")},${Math.min(weight * 0.9, 0.9)})`;
  return { filter, tint, vignette: a.vignette };
}

export const SPEED_PRESETS = [0.5, 1, 1.5, 2, 3];
export const MIN_SPEED = 0.1;
export const MAX_SPEED = 10;

/**
 * Where Instagram Reels and TikTok draw nothing over a vertical video, in canvas pixels: clear of the
 * top bar, the like and comment rail and the caption and buttons at the bottom. The same rule as the
 * engine's `Canvas::safe_area` (250 / 180 / 500 / 60 px of 1080×1920); null for other formats.
 */
export function safeArea(width: number, height: number): { left: number; top: number; right: number; bottom: number } | null {
  if (height < width * 1.7) return null;
  return { left: (width * 60) / 1080, top: (height * 250) / 1920, right: width - (width * 180) / 1080, bottom: height - (height * 500) / 1920 };
}
