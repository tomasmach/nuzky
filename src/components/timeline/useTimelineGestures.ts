import { useCallback, useEffect, useRef, useState, type PointerEvent, type RefObject } from "react";
import { followPointer } from "../../lib/drag";
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

/**
 * Moving and trimming clips, and scrubbing the ruler and empty lanes; both follow the pointer on
 * the window. A drag binds its listeners once when it starts and reads the latest zoom and
 * snapping from a ref, so pointer moves only update the drag state.
 */
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
  const [drag, setDrag] = useState<Drag | null>(null);
  const live = useRef({ project, zoom, snapping, minUs, timeAt });
  live.current = { project, zoom, snapping, minUs, timeAt };
  const stopDrag = useRef<(() => void) | null>(null);
  useEffect(() => () => stopDrag.current?.(), []);

  /** The drag after the pointer moved to `e`, or null while it has not moved far enough to count. */
  const step = useCallback(
    (d: Drag, e: globalThis.PointerEvent, kind: Track["kind"]): Drag | null => {
      const { project, zoom, snapping } = live.current;
      const moved = d.moved || Math.hypot(e.clientX - d.startX, e.clientY - d.startY) > 3;
      if (!moved || !project) return null;
      let dxUs = ((e.clientX - d.startX) / zoom) * US;
      let snapUs: number | null = null;
      const thr = (SNAP_PX / zoom) * US;
      if (snapping) {
        if (d.mode === "move") {
          const s = d.clip.startUs + dxUs;
          const a = snap(s, d.candidates, thr);
          const b = snap(s + d.clip.durationUs, d.candidates, thr);
          const da = a === null ? Infinity : Math.abs(a - s);
          const db = b === null ? Infinity : Math.abs(b - (s + d.clip.durationUs));
          if (a !== null && da <= db) {
            dxUs = a - d.clip.startUs;
            snapUs = a;
          } else if (b !== null) {
            dxUs = b - d.clip.durationUs - d.clip.startUs;
            snapUs = b;
          }
        } else {
          const edge = d.mode === "trimL" ? d.clip.startUs + dxUs : d.clip.startUs + d.clip.durationUs + dxUs;
          const s = snap(edge, d.candidates, thr);
          if (s !== null) {
            dxUs += s - edge;
            snapUs = s;
          }
        }
      }
      let target: string | null | undefined = undefined;
      if (d.mode === "move") {
        const rects = [...rows.current.entries()].map(([id, el]) => [id, el.getBoundingClientRect()] as const);
        const hit = rects.find(([, r]) => e.clientY >= r.top && e.clientY <= r.bottom);
        if (hit) {
          const t = project.tracks.find((tr) => tr.id === hit[0]);
          if (t && t.kind === kind && t.id !== d.trackId) target = t.id;
        } else if (rects.length > 0) {
          const top = Math.min(...rects.map(([, r]) => r.top));
          const bottom = Math.max(...rects.map(([, r]) => r.bottom));
          if ((kind !== "audio" && e.clientY < top) || (kind === "audio" && e.clientY > bottom)) target = null;
        }
      }
      return { ...d, moved: true, dxUs: Math.round(dxUs), snapUs, target };
    },
    [rows],
  );

  const finish = (d: Drag) => {
    const { select, edit } = useEditor.getState();
    if (!d.moved) {
      const sel = useEditor.getState().selection;
      if (d.shift) select(sel.includes(d.clip.id) ? sel.filter((id) => id !== d.clip.id) : [...sel, d.clip.id]);
      else select([d.clip.id]);
      return;
    }
    const r = dragResult(d, live.current.minUs);
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
  const finishRef = useRef(finish);
  finishRef.current = finish;

  const startClipDrag = useCallback(
    (e: PointerEvent, clip: Clip, track: Track) => {
      const { project } = live.current;
      if (e.button !== 0 || !project) return;
      e.stopPropagation();
      stopDrag.current?.();
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
      // While the AI edits, pressing a clip only selects it.
      const locked = !!useEditor.getState().aiRun;
      let d: Drag = { clip, trackId: track.id, mode, startX: e.clientX, startY: e.clientY, moved: false, shift: e.shiftKey, dxUs: 0, target: undefined, snapUs: null, candidates, maxDurUs, sourceInUs, speed: media?.speed ?? 1 };
      setDrag(d);

      // Pointer moves are handled once per display frame: a fast mouse sends several per frame, and each
      // one would re-render the timeline. The last one is applied before a release commits the drag.
      let frame = 0;
      let latest: globalThis.PointerEvent | null = null;
      const apply = () => {
        frame = 0;
        const ev = latest;
        latest = null;
        const next = ev && step(d, ev, track.kind);
        if (next) setDrag((d = next));
      };
      const end = (commit: boolean) => {
        unfollow();
        window.removeEventListener("keydown", onKey, true);
        cancelAnimationFrame(frame);
        if (commit) apply();
        stopDrag.current = null;
        setDrag(null);
        if (commit) finishRef.current(d);
      };
      const onKey = (ev: KeyboardEvent) => {
        if (ev.key === "Escape") {
          ev.stopPropagation();
          end(false);
        }
      };
      // A cancelled pointer or a lost window focus ends the drag like Esc: nothing changes.
      const unfollow = followPointer({
        move: (ev) => {
          if (locked) return;
          latest = ev;
          frame ||= requestAnimationFrame(apply);
        },
        up: () => end(true),
        cancel: () => end(false),
      });
      window.addEventListener("keydown", onKey, true);
      stopDrag.current = () => end(false);
    },
    [step],
  );

  const startScrub = useCallback((e: PointerEvent) => {
    if (e.button !== 0) return;
    const { seek } = useEditor.getState();
    seek(live.current.timeAt(e.clientX));
    let raf = 0;
    let lastX = e.clientX;
    followPointer({
      move: (ev) => {
        lastX = ev.clientX;
        if (!raf)
          raf = requestAnimationFrame(() => {
            raf = 0;
            seek(live.current.timeAt(lastX));
          });
      },
    });
  }, []);

  return { drag, startClipDrag, startScrub };
}
