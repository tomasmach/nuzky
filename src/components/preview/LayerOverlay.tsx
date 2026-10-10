import { useEffect, useRef, useState } from "react";
import { api } from "../../lib/api";
import { followPointer } from "../../lib/drag";
import { NO_CROP } from "../../lib/presets";
import { findClip, setClipTransform, transformAtPlayhead, useAiLocked, useEditor } from "../../lib/store";
import type { Crop, LayerBounds, Project, Transform } from "../../lib/types";
import { withEdge } from "../inspector/ShapeSection";

type Pt = [number, number];
type Edge = keyof Crop;
type Mode = "move" | "scale" | "rotate" | Edge;

/** Each crop handle sits in the middle of its edge of the box (top-left, top-right, bottom-right, bottom-left). */
const EDGE_CORNERS: { edge: Edge; from: number; to: number }[] = [
  { edge: "top", from: 0, to: 1 },
  { edge: "right", from: 1, to: 2 },
  { edge: "bottom", from: 2, to: 3 },
  { edge: "left", from: 3, to: 0 },
];

const HANDLE = 10;
const CROP_BAR = 18;
const ROTATE_GAP = 26;
const SNAP_SCREEN_PX = 6;

/** Convex quad test: the point is on the same side of every edge. */
function inQuad(p: Pt, q: Pt[]) {
  let sign = 0;
  for (let i = 0; i < q.length; i++) {
    const [ax, ay] = q[i];
    const [bx, by] = q[(i + 1) % q.length];
    const cross = (bx - ax) * (p[1] - ay) - (by - ay) * (p[0] - ax);
    if (cross !== 0) {
      if (sign !== 0 && Math.sign(cross) !== sign) return false;
      sign = Math.sign(cross);
    }
  }
  return true;
}

const centreOf = (q: Pt[]): Pt => [(q[0][0] + q[2][0]) / 2, (q[0][1] + q[2][1]) / 2];
const angleOf = (c: Pt, p: Pt) => (Math.atan2(p[1] - c[1], p[0] - c[0]) * 180) / Math.PI;

/** The part of a quad between the crop's edges. */
function cropQuad(q: Pt[], c: Crop): Pt[] {
  const at = (u: number, v: number): Pt => [q[0][0] + u * (q[1][0] - q[0][0]) + v * (q[3][0] - q[0][0]), q[0][1] + u * (q[1][1] - q[0][1]) + v * (q[3][1] - q[0][1])];
  return [at(c.left, c.top), at(1 - c.right, c.top), at(1 - c.right, 1 - c.bottom), at(c.left, 1 - c.bottom)];
}

/** Where `p` lies along the top and left edges of a quad, 0 at the top-left corner and 1 at the far ends. */
function alongQuad(q: Pt[], p: Pt): Pt {
  const along = (end: Pt) => {
    const [ex, ey] = [end[0] - q[0][0], end[1] - q[0][1]];
    return ((p[0] - q[0][0]) * ex + (p[1] - q[0][1]) * ey) / Math.max(1e-6, ex * ex + ey * ey);
  };
  return [along(q[1]), along(q[3])];
}

/** The corner radius of a video or image, as a share of half its shorter visible side. */
function shapeRadius(project: Project, clipId: string) {
  const content = findClip(project, clipId)?.clip.content;
  return content?.type === "media" ? (content.shape?.radius ?? 0) : 0;
}

/** Whether `p` is inside a quad with rounded corners, so a click beside a circle reaches the layer below. */
function inRounded(p: Pt, q: Pt[], radius: number) {
  if (radius <= 0) return true;
  const [u, v] = alongQuad(q, p);
  const [w, h] = [Math.hypot(q[1][0] - q[0][0], q[1][1] - q[0][1]), Math.hypot(q[3][0] - q[0][0], q[3][1] - q[0][1])];
  const r = (radius * Math.min(w, h)) / 2;
  const [x, y] = [Math.max(0, Math.abs(u - 0.5) * w - w / 2 + r), Math.max(0, Math.abs(v - 0.5) * h - h / 2 + r)];
  return Math.hypot(x, y) <= r;
}

function rotateAbout(p: Pt, c: Pt, deg: number): Pt {
  const r = (deg * Math.PI) / 180;
  const [dx, dy] = [p[0] - c[0], p[1] - c[1]];
  return [c[0] + dx * Math.cos(r) - dy * Math.sin(r), c[1] + dx * Math.sin(r) + dy * Math.cos(r)];
}

interface Gesture {
  mode: Mode;
  clipId: string;
  base: Transform;
  corners: Pt[];
  /** The whole layer before its crop. */
  frame: Pt[];
  start: Pt;
  key: string;
}

/**
 * The layers a selection box works on: the clips of the video at the playhead, or the texts of a cover.
 * Ids are `LayerBounds.clipId`.
 */
export interface LayerSource {
  /** The picture the layers sit on, in its own pixels. */
  canvas: { width: number; height: number };
  /** Where the layers are drawn now, bottom to top. */
  bounds: LayerBounds[];
  /** Where they are for a press: the clips ask the engine again, as the playhead may have moved. */
  current: () => Promise<LayerBounds[]>;
  /** Where they are once the gesture's last change is confirmed, so the box never jumps back. */
  settled: (last: Promise<unknown>) => Promise<LayerBounds[]>;
  onBounds: (bounds: LayerBounds[]) => void;
  selected: string | null;
  /** True when `id` is the one layer selected; a press on another layer selects it alone. */
  isOnlySelected: (id: string) => boolean;
  select: (id: string | null) => void;
  /** The layer's transform as the edit will change it, at the playhead for clips. */
  transform: (id: string) => Transform | null;
  change: (id: string, patch: Partial<Transform>, key: string) => Promise<unknown>;
  /** Videos and images crop; text does not. */
  croppable: (id: string) => boolean;
  /** Corner radius as a share of half the shorter visible side, so a click beside a circle reaches the layer below. */
  radius: (id: string) => number;
  /** While the AI edits, a click still selects but nothing moves. */
  locked: () => boolean;
  /** The box hides, e.g. while the video plays. */
  hidden: boolean;
}

/**
 * Selection box over the preview frame: click selects the top-most layer under the pointer,
 * dragging moves it, corners scale it, the top handle rotates it (Shift snaps to 15°). On videos and
 * images the bars in the middle of each edge crop that edge.
 * Draws nothing while the engine reports no layer bounds.
 *
 * `bleed` is how far the visible preview area reaches past the frame on each side. The box may
 * extend past the frame; handles that would leave the visible area are pinned to its edge.
 */
export function LayerOverlay({ width, height, bleed }: { width: number; height: number; bleed: { x: number; y: number } }) {
  const canvas = useEditor((s) => s.snap!.project.canvas);
  const revision = useEditor((s) => s.snap!.revision);
  const selection = useEditor((s) => s.selection);
  const playing = useEditor((s) => s.playing);
  // The box hides while playing, so the playhead only matters once paused.
  const timeUs = useEditor((s) => (s.playing ? null : s.timeUs));
  const [bounds, setBounds] = useState<LayerBounds[]>([]);
  // Re-rendered with the clip's content, so only the boolean is watched.
  const media = useEditor((s) => (selection.length === 1 ? findClip(s.snap!.project, selection[0])?.clip.content.type === "media" : false));

  useEffect(() => {
    if (timeUs === null) return;
    let alive = true;
    api
      .layerBounds(timeUs)
      .then((b) => alive && setBounds(b))
      .catch(() => alive && setBounds([]));
    return () => {
      alive = false;
    };
  }, [timeUs, revision]);

  const project = () => useEditor.getState().snap!.project;
  const source: LayerSource = {
    canvas,
    bounds,
    current: () => api.layerBounds(useEditor.getState().timeUs),
    settled: (last) => last.then(() => api.layerBounds(useEditor.getState().timeUs)),
    onBounds: setBounds,
    selected: selection.length === 1 ? selection[0] : null,
    isOnlySelected: (id) => {
      const sel = useEditor.getState().selection;
      return sel.length === 1 && sel[0] === id;
    },
    select: (id) => useEditor.getState().select(id ? [id] : []),
    transform: (id) => {
      const found = findClip(project(), id);
      return found ? transformAtPlayhead(found.clip, useEditor.getState().timeUs) : null;
    },
    // The store builds the edit from the latest confirmed clip, so a keyframe written earlier
    // in this gesture is updated, not duplicated, and other fields are never reverted.
    change: (id, patch, key) => setClipTransform(id, patch, key),
    croppable: (id) => id === selection[0] && media,
    radius: (id) => shapeRadius(project(), id),
    locked: () => !!useEditor.getState().aiRun,
    hidden: playing,
  };
  return <LayerBox width={width} height={height} bleed={bleed} source={source} />;
}

/** The selection box and its gestures over any picture of layers; see `LayerOverlay`. */
export function LayerBox({ width, height, bleed, source }: { width: number; height: number; bleed: { x: number; y: number }; source: LayerSource }) {
  const { canvas, bounds } = source;
  const locked = useAiLocked();
  const ref = useRef<HTMLDivElement>(null);
  const [live, setLive] = useState<{ corners: Pt[]; frame?: Pt[]; guideX: boolean; guideY: boolean } | null>(null);
  // Ends the running gesture's window listeners and frame request; also called on unmount.
  const stop = useRef<(() => void) | null>(null);
  // The gesture outlives renders; it reads the source as it is now.
  const src = useRef(source);
  src.current = source;
  const k = width / canvas.width;

  useEffect(() => () => stop.current?.(), []);

  const toCanvas = (x: number, y: number): Pt => {
    const r = ref.current!.getBoundingClientRect();
    return [(x - r.left) / k, (y - r.top) / k];
  };

  /** Changed transform fields and box for the pointer at `p` during gesture `g`. */
  const solve = (g: Gesture, p: Pt, shift: boolean): { patch: Partial<Transform>; corners: Pt[]; guideX: boolean; guideY: boolean } => {
    const c = centreOf(g.corners);
    // The layer scales and turns about the centre of the whole picture, which a crop moves off the box's centre.
    const pivot = centreOf(g.frame);
    const t = { ...g.base };
    let corners = g.corners;
    let guideX = false;
    let guideY = false;
    if (g.mode === "move") {
      // Snap the layer centre to the canvas centre lines.
      const tol = SNAP_SCREEN_PX / k;
      let dx = p[0] - g.start[0];
      let dy = p[1] - g.start[1];
      if (Math.abs(c[0] + dx - canvas.width / 2) < tol) {
        dx = canvas.width / 2 - c[0];
        guideX = true;
      }
      if (Math.abs(c[1] + dy - canvas.height / 2) < tol) {
        dy = canvas.height / 2 - c[1];
        guideY = true;
      }
      t.x = g.base.x + dx / canvas.width;
      t.y = g.base.y + dy / canvas.height;
      corners = g.corners.map(([x, y]) => [x + dx, y + dy]);
    } else if (g.mode === "scale") {
      const f = Math.hypot(p[0] - pivot[0], p[1] - pivot[1]) / Math.max(1, Math.hypot(g.start[0] - pivot[0], g.start[1] - pivot[1]));
      t.scale = Math.max(0.1, Math.min(10, g.base.scale * f));
      const ratio = t.scale / g.base.scale;
      corners = g.corners.map(([x, y]) => [pivot[0] + (x - pivot[0]) * ratio, pivot[1] + (y - pivot[1]) * ratio]);
    } else if (g.mode === "rotate") {
      let rot = g.base.rotation + angleOf(pivot, p) - angleOf(pivot, g.start);
      rot = ((((rot + 180) % 360) + 360) % 360) - 180;
      if (shift) rot = Math.round(rot / 15) * 15;
      t.rotation = Math.round(rot * 10) / 10;
      corners = g.corners.map((q) => rotateAbout(q, pivot, t.rotation - g.base.rotation));
    } else {
      // The dragged edge moves as far as the pointer, which may have grabbed a bar pinned to the edge of
      // the preview area; the picture itself stays put.
      const [[u, v], [u0, v0]] = [alongQuad(g.frame, p), alongQuad(g.frame, g.start)];
      const crop = g.base.crop ?? NO_CROP;
      const at = { left: crop.left + u - u0, right: crop.right - u + u0, top: crop.top + v - v0, bottom: crop.bottom - v + v0 }[g.mode];
      t.crop = withEdge(crop, g.mode, Math.round(at * 1000) / 1000);
      corners = cropQuad(g.frame, t.crop);
    }
    const patch = g.mode === "move" ? { x: t.x, y: t.y } : g.mode === "scale" ? { scale: t.scale } : g.mode === "rotate" ? { rotation: t.rotation } : { crop: t.crop };
    return { patch, corners, guideX, guideY };
  };

  const run = (g: Gesture, initial: { x: number; y: number; shift: boolean }) => {
    stop.current?.();
    let latest = initial;
    let raf = 0;
    let lastEdit: Promise<unknown> | null = null;
    const apply = () => {
      raf = 0;
      if (!ref.current) return;
      const { patch, corners, guideX, guideY } = solve(g, toCanvas(latest.x, latest.y), latest.shift);
      // While cropping, a dashed outline shows the whole layer.
      setLive({ corners, frame: g.mode in NO_CROP ? g.frame : undefined, guideX, guideY });
      lastEdit = src.current.change(g.clipId, patch, g.key);
    };
    const halt = () => {
      cancelAnimationFrame(raf);
      raf = 0;
      stop.current = null;
    };
    const settle = () => {
      if (!lastEdit) return setLive(null);
      // Keep the live box until the engine reports where the layer ended up, so it never jumps back.
      src.current
        .settled(lastEdit)
        .then((b) => ref.current && src.current.onBounds(b))
        .catch(() => undefined)
        .finally(() => ref.current && setLive(null));
    };
    const unfollow = followPointer({
      move: (e) => {
        latest = { x: e.clientX, y: e.clientY, shift: e.shiftKey };
        if (!raf) raf = requestAnimationFrame(apply);
      },
      up: () => {
        const pending = raf !== 0;
        halt();
        if (pending) apply();
        settle();
      },
      // The system took the pointer (e.g. a touch turned into a scroll): keep what was applied.
      cancel: () => {
        halt();
        settle();
      },
    });
    stop.current = () => {
      unfollow();
      halt();
    };
  };

  const begin = (mode: Mode, b: LayerBounds, e: { clientX: number; clientY: number }) => {
    const base = src.current.transform(b.clipId);
    if (!base) return null;
    return { mode, clipId: b.clipId, corners: b.corners as Pt[], frame: b.frame as Pt[], base, start: toCanvas(e.clientX, e.clientY), key: `preview:${b.clipId}:${Date.now()}` };
  };

  // Moves that arrive while the bounds request is in flight are replayed once it resolves.
  const onPointerDown = async (e: React.PointerEvent) => {
    if (e.button !== 0 || source.hidden) return;
    const down = { clientX: e.clientX, clientY: e.clientY };
    let latest = { x: e.clientX, y: e.clientY, shift: e.shiftKey };
    let released = false;
    const release = () => (released = true);
    stop.current?.();
    const unfollow = followPointer({ move: (ev) => (latest = { x: ev.clientX, y: ev.clientY, shift: ev.shiftKey }), up: release, cancel: release });
    const detach = () => {
      unfollow();
      stop.current = null;
    };
    stop.current = detach;
    let list: LayerBounds[] = [];
    try {
      list = await src.current.current();
    } catch {
      /* no bounds: nothing to select */
    }
    // Unmounted or replaced by another gesture while waiting.
    if (stop.current !== detach) return;
    detach();
    if (!ref.current) return;
    const at = src.current;
    at.onBounds(list);
    if (list.length === 0) return;
    const p = toCanvas(down.clientX, down.clientY);
    const hit = [...list].reverse().find((b) => inQuad(p, b.corners as Pt[]) && inRounded(p, b.corners as Pt[], at.radius(b.clipId)));
    if (!hit) {
      at.select(null);
      return;
    }
    if (!at.isOnlySelected(hit.clipId)) at.select(hit.clipId);
    // While the AI edits, a click still selects but nothing moves.
    if (released || at.locked()) return;
    const g = begin("move", hit, down);
    if (g) run(g, latest);
  };

  const onHandle = (mode: Mode, b: LayerBounds) => (e: React.PointerEvent) => {
    if (e.button !== 0 || src.current.locked()) return;
    e.stopPropagation();
    const g = begin(mode, b, e);
    if (g) run(g, { x: e.clientX, y: e.clientY, shift: e.shiftKey });
  };

  const selected = source.selected === null ? undefined : bounds.find((b) => b.clipId === source.selected);
  const corners = live?.corners ?? (selected?.corners as Pt[] | undefined);
  const screen = corners?.map(([x, y]) => [x * k, y * k] as Pt);
  const croppable = !!selected && source.croppable(selected.clipId);
  // Visible preview area in overlay coordinates; the preview clips everything outside it.
  const pad = HANDLE / 2 + 1;
  const visible = (p: Pt) => p[0] >= -bleed.x + pad && p[0] <= width + bleed.x - pad && p[1] >= -bleed.y + pad && p[1] <= height + bleed.y - pad;
  const pin = (p: Pt): Pt => [
    Math.max(-bleed.x + pad, Math.min(width + bleed.x - pad, p[0])),
    Math.max(-bleed.y + pad, Math.min(height + bleed.y - pad, p[1])),
  ];
  // Rotation handle sits above the middle of the top edge, along the box's own "up". When that
  // would leave the visible area (a layer filling the canvas), it moves just inside the edge
  // instead, and as a last resort it is pinned to the edge like the corner handles.
  const top = screen && ([(screen[0][0] + screen[1][0]) / 2, (screen[0][1] + screen[1][1]) / 2] as Pt);
  const c = screen && centreOf(screen);
  const up =
    top &&
    c &&
    (() => {
      const len = Math.hypot(top[0] - c[0], top[1] - c[1]) || 1;
      const dir: Pt = [(top[0] - c[0]) / len, (top[1] - c[1]) / len];
      const out: Pt = [top[0] + dir[0] * ROTATE_GAP, top[1] + dir[1] * ROTATE_GAP];
      if (visible(out)) return out;
      // Inside a small box the handle would cover the picture and catch the press meant to move it.
      if (2 * len < 3 * ROTATE_GAP) return pin(out);
      const inside: Pt = [top[0] - dir[0] * ROTATE_GAP, top[1] - dir[1] * ROTATE_GAP];
      return visible(inside) ? inside : pin(inside);
    })();
  const guide = "pointer-events-none absolute bg-warn shadow-[0_0_0_1px_rgba(0,0,0,0.55)]";

  return (
    <div ref={ref} className="absolute inset-0" style={{ cursor: selected && !locked ? "move" : "default" }} onPointerDown={onPointerDown} data-testid="layer-overlay">
      {live?.guideX && <div className={`${guide} inset-y-0 left-1/2 w-px`} />}
      {live?.guideY && <div className={`${guide} inset-x-0 top-1/2 h-px`} />}
      {screen && selected && !source.hidden && top && up && (
        <>
          <svg className="pointer-events-none absolute inset-0 overflow-visible" width={width} height={height}>
            {live?.frame && (
              <polygon points={live.frame.map(([x, y]) => `${x * k},${y * k}`).join(" ")} fill="none" stroke="var(--color-fg)" strokeOpacity={0.6} strokeWidth={1} strokeDasharray="4 4" />
            )}
            <polygon points={screen.map((p) => p.join(",")).join(" ")} fill="none" stroke="var(--color-fg)" strokeWidth={1.5} />
            <line x1={top[0]} y1={top[1]} x2={up[0]} y2={up[1]} stroke="var(--color-fg)" strokeWidth={1.5} />
          </svg>
          {!locked && screen.map(pin).map(([x, y], i) => (
            <div
              key={i}
              role="presentation"
              data-handle="scale"
              onPointerDown={onHandle("scale", selected)}
              className="absolute rounded-[2px] border border-black/60 bg-fg"
              style={{ left: x - HANDLE / 2, top: y - HANDLE / 2, width: HANDLE, height: HANDLE, cursor: i % 2 === 0 ? "nwse-resize" : "nesw-resize" }}
            />
          ))}
          {!locked &&
            croppable &&
            EDGE_CORNERS.map(({ edge, from, to }) => {
              // On a short edge the bar would cover the corner handles; the inspector still crops it.
              if (Math.hypot(screen[to][0] - screen[from][0], screen[to][1] - screen[from][1]) < CROP_BAR + 2 * HANDLE + 8) return null;
              const [x, y] = pin([(screen[from][0] + screen[to][0]) / 2, (screen[from][1] + screen[to][1]) / 2]);
              const angle = angleOf(screen[from], screen[to]);
              return (
                <div
                  key={edge}
                  role="presentation"
                  title={`Drag to crop the ${edge} edge`}
                  data-handle={`crop-${edge}`}
                  onPointerDown={onHandle(edge, selected)}
                  className="absolute rounded-full border border-black/60 bg-fg"
                  style={{
                    left: x - CROP_BAR / 2,
                    top: y - 2.5,
                    width: CROP_BAR,
                    height: 5,
                    transform: `rotate(${angle}deg)`,
                    cursor: edge === "left" || edge === "right" ? "ew-resize" : "ns-resize",
                  }}
                />
              );
            })}
          {!locked && <div
            role="presentation"
            title="Rotate (Shift snaps to 15°)"
            data-handle="rotate"
            onPointerDown={onHandle("rotate", selected)}
            className="absolute rounded-full border border-black/60 bg-fg"
            style={{ left: up[0] - 6, top: up[1] - 6, width: 12, height: 12, cursor: "grab" }}
          />}
        </>
      )}
    </div>
  );
}
