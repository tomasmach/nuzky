import captionStyles from "../../assets/presets/captions.json";
import type { Adjust, Animation, AnimationKind, CaptionPreset, Crop, KeywordPick, Shape, TextStyle, Transform, TransitionKind } from "./types";

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
export const NO_CROP: Crop = { left: 0, top: 0, right: 0, bottom: 0 };
export const NO_SHAPE: Shape = { radius: 0, borderWidth: 0, borderColor: "#ffffff", shadow: 0 };
export const NO_ADJUST: Adjust = { exposure: 0, tint: 0, highlights: 0, shadows: 0, fade: 0, brightness: 0, contrast: 0, saturation: 0, temperature: 0, vignette: 0 };

export const ADJUST_ROWS: { key: keyof Adjust; label: string; min: number }[] = [
  { key: "exposure", label: "Exposure", min: -100 },
  { key: "brightness", label: "Brightness", min: -100 },
  { key: "contrast", label: "Contrast", min: -100 },
  { key: "highlights", label: "Highlights", min: -100 },
  { key: "shadows", label: "Shadows", min: -100 },
  { key: "saturation", label: "Saturation", min: -100 },
  { key: "temperature", label: "Temperature", min: -100 },
  { key: "tint", label: "Tint", min: -100 },
  { key: "fade", label: "Fade", min: 0 },
  { key: "vignette", label: "Vignette", min: 0 },
];

export const TEXT_PRESETS: { name: string; text: string; style: TextStyle }[] = [
  { name: "Classic", text: "Your text", style: { fontSize: 84, color: "#ffffff", bold: true, strokeWidth: 7, strokeColor: "#000000", background: null } },
  { name: "Title", text: "BIG TITLE", style: { fontSize: 130, color: "#ffffff", bold: true, strokeWidth: 0, strokeColor: "#000000", background: null } },
  { name: "Label", text: "Label", style: { fontSize: 72, color: "#111111", bold: true, strokeWidth: 0, strokeColor: "#000000", background: "#ffffffee" } },
  { name: "Highlight", text: "Highlight", style: { fontSize: 84, color: "#111111", bold: true, strokeWidth: 0, strokeColor: "#000000", background: "#ffd400ee" } },
  { name: "Neon", text: "Neon", style: { fontSize: 96, color: "#9cffd9", bold: true, strokeWidth: 5, strokeColor: "#0b6b4f", background: null } },
  { name: "Plain", text: "Plain text", style: { fontSize: 64, color: "#ffffff", bold: false, strokeWidth: 0, strokeColor: "#000000", background: null } },
];

/**
 * Caption styles, shared with the engine and MCP (assets/presets/captions.json); Reel comes first. A style
 * without a font keeps the font the captions have, so a font picked separately survives it.
 */
export const CAPTION_STYLES = captionStyles as CaptionPreset[];

const lower = (c: string | null | undefined) => (c ?? "").toLowerCase();

/** Same look apart from the font; a karaoke highlight and key words count, so Karaoke is not Reel. */
export function sameStyle(a: TextStyle, b: TextStyle) {
  return (
    a.fontSize === b.fontSize &&
    lower(a.color) === lower(b.color) &&
    a.bold === b.bold &&
    a.strokeWidth === b.strokeWidth &&
    lower(a.strokeColor) === lower(b.strokeColor) &&
    lower(a.background) === lower(b.background) &&
    lower(a.highlight) === lower(b.highlight) &&
    lower(a.keywords?.color) === lower(b.keywords?.color) &&
    (a.keywords?.pick ?? null) === (b.keywords?.pick ?? null)
  );
}

const sameAnimation = (a: Animation | null | undefined, b: Animation | null | undefined) => a?.kind === b?.kind && (a?.durationUs ?? 0) === (b?.durationUs ?? 0);

/** Whether captions in `style` with these animations look like `preset`: its font counts only when it brings one. */
export function isLook(preset: CaptionPreset, style: TextStyle, animIn: Animation | null | undefined, animOut: Animation | null | undefined) {
  return (
    sameStyle(preset.style, style) &&
    (!preset.style.fontFamily || preset.style.fontFamily === style.fontFamily) &&
    sameAnimation(preset.animIn, animIn) &&
    sameAnimation(preset.animOut, animOut)
  );
}

/** Which words of a caption the key word colour marks. */
export const KEYWORD_PICKS: { id: KeywordPick | "off"; label: string; title: string }[] = [
  { id: "off", label: "Off", title: "No key words" },
  { id: "emphasis", label: "Emphasis", title: "Numbers and words said louder than the rest" },
  { id: "first", label: "First", title: "The first word of every caption" },
  { id: "last", label: "Last", title: "The last word of every caption" },
  { id: "longest", label: "Longest", title: "The longest word of every caption" },
];
export const KEYWORD_COLOR = "#ffd400";

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
