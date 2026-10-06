import { useEffect, useState, type PointerEvent, type RefObject } from "react";
import { allClips, useEditor } from "../../lib/store";
import { US } from "../../lib/time";
import type { Clip, Project, Track } from "../../lib/types";
import { EDGE, clipAsset } from "./ClipView";

const SNAP_PX = 8;

type Mode = "move" | "trimL" | "trimR";

export interface Drag {
  clip: Clip;
  trackId: string;
  mode: Mode;
  startX: number;
  startY: number;
  moved: boolean;
  shift: boolean;
  dxUs: number;
  /** undefined: same track, null: a new track, string: another track. */
  target: string | null | undefined;
  snapUs: number | null;
  candidates: number[];
  maxDurUs: number | null;
  sourceInUs: number | null;
  speed: number;
}

/**
 * Clip timing while a drag is in progress. Every value is whole microseconds (the engine takes
 * integers), rounded towards the inside of the source the same way the engine clamps a trim.
 */
export function dragResult(d: Drag, minUs: number): { startUs: number; durationUs: number } {
  const { clip } = d;
  if (d.mode === "move") return { startUs: Math.max(0, clip.startUs + d.dxUs), durationUs: clip.durationUs };
  if (d.mode === "trimL") {
    // The left edge stops where the source starts, at this clip's speed.
    const lower = d.sourceInUs !== null ? Math.ceil(-d.sourceInUs / d.speed) : -clip.startUs;
    const delta = Math.min(Math.max(d.dxUs, lower, -clip.startUs), clip.durationUs - minUs);
    return { startUs: clip.startUs + delta, durationUs: clip.durationUs - delta };
  }
  const dur = Math.max(minUs, Math.min(clip.durationUs + d.dxUs, d.maxDurUs ?? Infinity));
  return { startUs: clip.startUs, durationUs: dur };
}

function snap(value: number, candidates: number[], thr: number): number | null {
  let best: number | null = null;
  for (const c of candidates) if (Math.abs(c - value) <= thr && (best === null || Math.abs(c - value) < Math.abs(best - value))) best = c;
  return best;
}

/** Moving and trimming clips, and scrubbing the ruler and empty lanes; both follow the pointer on the window. */
export function useTimelineGestures({
  project,
  zoom,
  snapping,
  minUs,
  rows,
  timeAt,
}: {
  project: Project | undefined;
  zoom: number;
  snapping: boolean;
  minUs: number;
  rows: RefObject<Map<string, HTMLDivElement>>;
  timeAt: (clientX: number) => number;
}) {
  const { select, seek, edit } = useEditor.getState();
  const [drag, setDrag] = useState<Drag | null>(null);

  const startClipDrag = (e: PointerEvent, clip: Clip, track: Track) => {
    if (e.button !== 0 || !project) return;
    e.stopPropagation();
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const x = e.clientX - rect.left;
    const mode: Mode = x <= EDGE ? "trimL" : x >= rect.width - EDGE ? "trimR" : "move";
    const asset = clipAsset(project, clip);
    const media = clip.content.type === "media" ? clip.content : null;
    const sourceInUs = media && asset?.kind !== "image" ? media.sourceInUs : null;
    // The right edge stops where the source ends, at this clip's speed (floored like the engine).
    const maxDurUs = sourceInUs !== null && asset && media ? Math.floor((asset.durationUs - sourceInUs) / media.speed) : null;
    const candidates = [0, useEditor.getState().timeUs];
    for (const c of allClips(project)) if (c.id !== clip.id) candidates.push(c.startUs, c.startUs + c.durationUs);
    setDrag({ clip, trackId: track.id, mode, startX: e.clientX, startY: e.clientY, moved: false, shift: e.shiftKey, dxUs: 0, target: undefined, snapUs: null, candidates, maxDurUs, sourceInUs, speed: media?.speed ?? 1 });
  };

  useEffect(() => {
    if (!drag || !project) return;
    const kind = project.tracks.find((t) => t.id === drag.trackId)?.kind ?? "video";
    const onMove = (e: globalThis.PointerEvent) => {
      const moved = drag.moved || Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) > 3;
      if (!moved) return;
      let dxUs = ((e.clientX - drag.startX) / zoom) * US;
      let snapUs: number | null = null;
      const thr = (SNAP_PX / zoom) * US;
      if (snapping) {
        if (drag.mode === "move") {
          const s = drag.clip.startUs + dxUs;
          const a = snap(s, drag.candidates, thr);
          const b = snap(s + drag.clip.durationUs, drag.candidates, thr);
          const da = a === null ? Infinity : Math.abs(a - s);
          const db = b === null ? Infinity : Math.abs(b - (s + drag.clip.durationUs));
          if (a !== null && da <= db) {
            dxUs = a - drag.clip.startUs;
            snapUs = a;
          } else if (b !== null) {
            dxUs = b - drag.clip.durationUs - drag.clip.startUs;
            snapUs = b;
          }
        } else {
          const edge = drag.mode === "trimL" ? drag.clip.startUs + dxUs : drag.clip.startUs + drag.clip.durationUs + dxUs;
          const s = snap(edge, drag.candidates, thr);
          if (s !== null) {
            dxUs += s - edge;
            snapUs = s;
          }
        }
      }
      let target: string | null | undefined = undefined;
      if (drag.mode === "move") {
        const entries = [...rows.current.entries()];
        const rects = entries.map(([id, el]) => [id, el.getBoundingClientRect()] as const);
        const hit = rects.find(([, r]) => e.clientY >= r.top && e.clientY <= r.bottom);
        if (hit) {
          const t = project.tracks.find((tr) => tr.id === hit[0]);
          if (t && t.kind === kind && t.id !== drag.trackId) target = t.id;
        } else if (rects.length > 0) {
          const top = Math.min(...rects.map(([, r]) => r.top));
          const bottom = Math.max(...rects.map(([, r]) => r.bottom));
          if ((kind !== "audio" && e.clientY < top) || (kind === "audio" && e.clientY > bottom)) target = null;
        }
      }
      setDrag({ ...drag, moved: true, dxUs: Math.round(dxUs), snapUs, target });
    };
    const finish = (commit: boolean) => {
      const d = drag;
      setDrag(null);
      if (!commit) return;
      if (!d.moved) {
        const sel = useEditor.getState().selection;
        if (d.shift) select(sel.includes(d.clip.id) ? sel.filter((id) => id !== d.clip.id) : [...sel, d.clip.id]);
        else select([d.clip.id]);
        return;
      }
      const r = dragResult(d, minUs);
      if (d.mode === "move") {
        const trackId = d.target === undefined ? d.trackId : d.target;
        if (trackId === d.trackId && r.startUs === d.clip.startUs) return;
        edit({ type: "moveClip", clipId: d.clip.id, trackId, startUs: r.startUs });
      } else {
        const sourceInUs = d.mode === "trimL" && d.sourceInUs !== null ? Math.max(0, d.sourceInUs + Math.round((r.startUs - d.clip.startUs) * d.speed)) : null;
        edit({ type: "trimClip", clipId: d.clip.id, startUs: r.startUs, durationUs: r.durationUs, sourceInUs });
      }
      select([d.clip.id]);
    };
    const onUp = () => finish(true);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        finish(false);
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [drag, project, zoom, snapping, minUs, edit, select, rows]);

  const startScrub = (e: PointerEvent) => {
    if (e.button !== 0) return;
    seek(timeAt(e.clientX));
    let raf = 0;
    let lastX = e.clientX;
    const move = (ev: globalThis.PointerEvent) => {
      lastX = ev.clientX;
      if (!raf)
        raf = requestAnimationFrame(() => {
          raf = 0;
          seek(timeAt(lastX));
        });
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return { drag, startClipDrag, startScrub };
}
