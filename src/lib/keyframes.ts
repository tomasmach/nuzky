import { LIMITS } from "./limits";
import { NO_CROP } from "./presets";
import type { Canvas, Clip, Ease, Keyframe, MotionKind, Transform } from "./types";
import { US } from "./time";

/** Keyframes closer than half a frame to the playhead count as "at the playhead". */
export function keyframeTolerance(fps: number) {
  return US / fps / 2;
}

/** Share of the way to the next keyframe at `p` of the time between them, as the engine's `Ease::at`. */
function eased(ease: Ease, p: number) {
  return ease === "smooth" ? p * p * (3 - 2 * p) : p;
}

function lerp(a: Transform, b: Transform, f: number): Transform {
  const m = (x: number, y: number) => x + (y - x) * f;
  const t: Transform = { x: m(a.x, b.x), y: m(a.y, b.y), scale: m(a.scale, b.scale), rotation: m(a.rotation, b.rotation), opacity: m(a.opacity, b.opacity) };
  // A keyframe without a crop shows the whole layer.
  if (!a.crop && !b.crop) return t;
  const [ca, cb] = [a.crop ?? NO_CROP, b.crop ?? NO_CROP];
  return { ...t, crop: { left: m(ca.left, cb.left), top: m(ca.top, cb.top), right: m(ca.right, cb.right), bottom: m(ca.bottom, cb.bottom) } };
}

/** Same rule as the renderer: along each keyframe's ease to the next, nearest one holds outside them. */
export function transformAt(clip: Clip, base: Transform, offsetUs: number): Transform {
  const ks = clip.keyframes;
  if (ks.length === 0) return base;
  if (offsetUs <= ks[0].tUs) return ks[0].transform;
  const last = ks[ks.length - 1];
  if (offsetUs >= last.tUs) return last.transform;
  const i = ks.findIndex((k) => k.tUs > offsetUs);
  const a = ks[i - 1];
  const b = ks[i];
  return lerp(a.transform, b.transform, eased(a.ease, (offsetUs - a.tUs) / (b.tUs - a.tUs)));
}

/** Playhead position inside the clip, clamped to its length. */
export function clipOffset(clip: Clip, timeUs: number) {
  return Math.round(Math.max(0, Math.min(clip.durationUs, timeUs - clip.startUs)));
}

export function keyframeIndexAt(clip: Clip, offsetUs: number, tol: number) {
  return clip.keyframes.findIndex((k) => Math.abs(k.tUs - offsetUs) <= tol);
}

/**
 * Replaces the keyframe at `offsetUs` or inserts one, keeping the list sorted. A new keyframe
 * takes the curve of the one before it, so a smooth move stays smooth.
 */
export function upsertKeyframe(clip: Clip, offsetUs: number, transform: Transform, tol: number): Keyframe[] {
  const i = keyframeIndexAt(clip, offsetUs, tol);
  if (i >= 0) return clip.keyframes.map((k, j) => (j === i ? { ...k, transform } : k));
  const before = clip.keyframes.findLast((k) => k.tUs < offsetUs) ?? clip.keyframes[0];
  return [...clip.keyframes, { tUs: offsetUs, transform, ease: before?.ease ?? "linear" }].sort((a, b) => a.tUs - b.tUs);
}

/** The canvas point a motion preset zooms about, as the engine's `motion_centre`. */
function motionCentre(canvas: Canvas, kind: MotionKind): [number, number] {
  const { width: w, height: h } = canvas;
  const { left, top, right, bottom } = canvas.safeArea ?? { left: 0, top: 0, right: w, bottom: h };
  return [(kind === "kenBurns" ? right : (left + right) / 2) / w - 0.5, (top + bottom) / 2 / h - 0.5];
}

const close = (a: Transform, b: Transform) =>
  (["x", "y", "scale", "rotation", "opacity"] as const).every((k) => Math.abs(a[k] - b[k]) < 1e-4);

/**
 * The motion preset the clip's keyframes are, with its strength, or null for none or keyframes of
 * the user's own: two smooth keyframes from the clip's transform, zoomed about the preset's centre
 * by a strength the engine accepts.
 */
export function motionOf(clip: Clip, canvas: Canvas): { kind: MotionKind; strength: number } | null {
  const ks = clip.keyframes;
  const base = clip.content.transform;
  if (ks.length !== 2 || ks.some((k) => k.ease !== "smooth")) return null;
  for (const kind of ["pushIn", "pullOut", "kenBurns"] as const) {
    const [still, zoomed] = kind === "pullOut" ? [ks[1], ks[0]] : [ks[0], ks[1]];
    const s = zoomed.transform.scale / base.scale;
    const [fx, fy] = motionCentre(canvas, kind);
    const expected = { ...base, x: base.x + (s - 1) * (base.x - fx), y: base.y + (s - 1) * (base.y - fy), scale: base.scale * s };
    const strength = Math.round((s - 1) * 1000) / 1000;
    const valid = strength >= LIMITS.minMotionStrength && strength <= LIMITS.maxMotionStrength;
    if (valid && close(still.transform, base) && close(zoomed.transform, expected)) return { kind, strength };
  }
  return null;
}
