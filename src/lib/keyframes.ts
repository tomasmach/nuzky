import { NO_CROP } from "./presets";
import type { Clip, Keyframe, Transform } from "./types";
import { US } from "./time";

/** Keyframes closer than half a frame to the playhead count as "at the playhead". */
export function keyframeTolerance(fps: number) {
  return US / fps / 2;
}

function lerp(a: Transform, b: Transform, f: number): Transform {
  const m = (x: number, y: number) => x + (y - x) * f;
  const t: Transform = { x: m(a.x, b.x), y: m(a.y, b.y), scale: m(a.scale, b.scale), rotation: m(a.rotation, b.rotation), opacity: m(a.opacity, b.opacity) };
  // A keyframe without a crop shows the whole layer.
  if (!a.crop && !b.crop) return t;
  const [ca, cb] = [a.crop ?? NO_CROP, b.crop ?? NO_CROP];
  return { ...t, crop: { left: m(ca.left, cb.left), top: m(ca.top, cb.top), right: m(ca.right, cb.right), bottom: m(ca.bottom, cb.bottom) } };
}

/** Same rule as the renderer: linear between keyframes, nearest one holds outside them. */
export function transformAt(clip: Clip, base: Transform, offsetUs: number): Transform {
  const ks = clip.keyframes;
  if (ks.length === 0) return base;
  if (offsetUs <= ks[0].tUs) return ks[0].transform;
  const last = ks[ks.length - 1];
  if (offsetUs >= last.tUs) return last.transform;
  const i = ks.findIndex((k) => k.tUs > offsetUs);
  const a = ks[i - 1];
  const b = ks[i];
  return lerp(a.transform, b.transform, (offsetUs - a.tUs) / (b.tUs - a.tUs));
}

/** Playhead position inside the clip, clamped to its length. */
export function clipOffset(clip: Clip, timeUs: number) {
  return Math.round(Math.max(0, Math.min(clip.durationUs, timeUs - clip.startUs)));
}

export function keyframeIndexAt(clip: Clip, offsetUs: number, tol: number) {
  return clip.keyframes.findIndex((k) => Math.abs(k.tUs - offsetUs) <= tol);
}

/** Replaces the keyframe at `offsetUs` or inserts one, keeping the list sorted. */
export function upsertKeyframe(clip: Clip, offsetUs: number, transform: Transform, tol: number): Keyframe[] {
  const i = keyframeIndexAt(clip, offsetUs, tol);
  if (i >= 0) return clip.keyframes.map((k, j) => (j === i ? { ...k, transform } : k));
  return [...clip.keyframes, { tUs: offsetUs, transform }].sort((a, b) => a.tUs - b.tUs);
}
