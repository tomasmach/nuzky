import { useEffect, useRef, useState } from "react";
import { api } from "../../lib/api";
import { findClip, setClipTransform, transformAtPlayhead, useEditor } from "../../lib/store";
import type { LayerBounds, Transform } from "../../lib/types";

type Pt = [number, number];
type Mode = "move" | "scale" | "rotate";

const HANDLE = 10;
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
  start: Pt;
  key: string;
}

/**
 * Selection box over the preview frame: click selects the top-most layer under the pointer,
 * dragging moves it, corners scale it, the top handle rotates it (Shift snaps to 15°).
 * Draws nothing while the engine reports no layer bounds.
 */
export function LayerOverlay({ width, height }: { width: number; height: number }) {
  const canvas = useEditor((s) => s.snap!.project.canvas);
  const revision = useEditor((s) => s.snap!.revision);
  const selection = useEditor((s) => s.selection);
  const timeUs = useEditor((s) => s.timeUs);
  const playing = useEditor((s) => s.playing);
  const ref = useRef<HTMLDivElement>(null);
  const [bounds, setBounds] = useState<LayerBounds[]>([]);
  const [live, setLive] = useState<{ corners: Pt[]; guideX: boolean; guideY: boolean } | null>(null);
  const k = width / canvas.width;

  useEffect(() => {
    if (playing) return;
    let alive = true;
    api
      .layerBounds(timeUs)
      .then((b) => alive && setBounds(b))
      .catch(() => alive && setBounds([]));
    return () => {
      alive = false;
    };
  }, [timeUs, revision, playing]);

  const toCanvas = (x: number, y: number): Pt => {
    const r = ref.current!.getBoundingClientRect();
    return [(x - r.left) / k, (y - r.top) / k];
  };

  /** Transform and box for the pointer at `p` during gesture `g`. */
  const solve = (g: Gesture, p: Pt, shift: boolean) => {
    const c = centreOf(g.corners);
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
      const f = Math.hypot(p[0] - c[0], p[1] - c[1]) / Math.max(1, Math.hypot(g.start[0] - c[0], g.start[1] - c[1]));
      t.scale = Math.max(0.1, Math.min(10, g.base.scale * f));
      const ratio = t.scale / g.base.scale;
      corners = g.corners.map(([x, y]) => [c[0] + (x - c[0]) * ratio, c[1] + (y - c[1]) * ratio]);
    } else {
      let rot = g.base.rotation + angleOf(c, p) - angleOf(c, g.start);
      rot = ((((rot + 180) % 360) + 360) % 360) - 180;
      if (shift) rot = Math.round(rot / 15) * 15;
      t.rotation = Math.round(rot * 10) / 10;
      corners = g.corners.map((q) => rotateAbout(q, c, t.rotation - g.base.rotation));
    }
    return { t, corners, guideX, guideY };
  };

  const run = (g: Gesture, initial: { x: number; y: number; shift: boolean }) => {
    let latest = initial;
    let raf = 0;
    let lastEdit: Promise<unknown> | null = null;
    const apply = () => {
      raf = 0;
      const { t, corners, guideX, guideY } = solve(g, toCanvas(latest.x, latest.y), latest.shift);
      setLive({ corners, guideX, guideY });
      // Re-read the clip so a keyframe written earlier in this gesture is updated, not duplicated.
      const found = findClip(useEditor.getState().snap!.project, g.clipId);
      if (found) lastEdit = setClipTransform(found.clip, t, g.key);
    };
    const move = (e: PointerEvent) => {
      latest = { x: e.clientX, y: e.clientY, shift: e.shiftKey };
      if (!raf) raf = requestAnimationFrame(apply);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (raf) {
        cancelAnimationFrame(raf);
        apply();
      }
      if (!lastEdit) return;
      // Keep the live box until the engine reports where the layer ended up, so it never jumps back.
      lastEdit
        .then(() => api.layerBounds(useEditor.getState().timeUs))
        .then(setBounds)
        .catch(() => undefined)
        .finally(() => setLive(null));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const begin = (mode: Mode, clipId: string, corners: Pt[], e: { clientX: number; clientY: number }) => {
    const found = findClip(useEditor.getState().snap!.project, clipId);
    if (!found) return null;
    return { mode, clipId, corners, base: transformAtPlayhead(found.clip, useEditor.getState().timeUs), start: toCanvas(e.clientX, e.clientY), key: `preview:${clipId}:${Date.now()}` };
  };

  // Moves that arrive while the bounds request is in flight are replayed once it resolves.
  const onPointerDown = async (e: React.PointerEvent) => {
    if (e.button !== 0 || playing) return;
    const down = { clientX: e.clientX, clientY: e.clientY };
    let latest = { x: e.clientX, y: e.clientY, shift: e.shiftKey };
    let released = false;
    const track = (ev: PointerEvent) => (latest = { x: ev.clientX, y: ev.clientY, shift: ev.shiftKey });
    const release = () => (released = true);
    window.addEventListener("pointermove", track);
    window.addEventListener("pointerup", release);
    let list: LayerBounds[] = [];
    try {
      list = await api.layerBounds(useEditor.getState().timeUs);
    } catch {
      /* no bounds: nothing to select */
    }
    window.removeEventListener("pointermove", track);
    window.removeEventListener("pointerup", release);
    setBounds(list);
    if (list.length === 0) return;
    const p = toCanvas(down.clientX, down.clientY);
    const hit = [...list].reverse().find((b) => inQuad(p, b.corners as Pt[]));
    if (!hit) {
      useEditor.getState().select([]);
      return;
    }
    if (!useEditor.getState().selection.includes(hit.clipId) || useEditor.getState().selection.length > 1) useEditor.getState().select([hit.clipId]);
    if (released) return;
    const g = begin("move", hit.clipId, hit.corners as Pt[], down);
    if (g) run(g, latest);
  };

  const onHandle = (mode: Mode, clipId: string, corners: Pt[]) => (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    const g = begin(mode, clipId, corners, e);
    if (g) run(g, { x: e.clientX, y: e.clientY, shift: e.shiftKey });
  };

  const selected = selection.length === 1 ? bounds.find((b) => b.clipId === selection[0]) : undefined;
  const corners = live?.corners ?? (selected?.corners as Pt[] | undefined);
  const screen = corners?.map(([x, y]) => [x * k, y * k] as Pt);
  // Rotation handle sits above the middle of the top edge, along the box's own "up".
  const top = screen && ([(screen[0][0] + screen[1][0]) / 2, (screen[0][1] + screen[1][1]) / 2] as Pt);
  const c = screen && centreOf(screen);
  const up = top && c && (() => {
    const len = Math.hypot(top[0] - c[0], top[1] - c[1]) || 1;
    return [top[0] + ((top[0] - c[0]) / len) * ROTATE_GAP, top[1] + ((top[1] - c[1]) / len) * ROTATE_GAP] as Pt;
  })();

  return (
    <div ref={ref} className="absolute inset-0" style={{ cursor: selected ? "move" : "default" }} onPointerDown={onPointerDown} data-testid="layer-overlay">
      {live?.guideX && <div className="pointer-events-none absolute inset-y-0 left-1/2 w-px bg-warn" />}
      {live?.guideY && <div className="pointer-events-none absolute inset-x-0 top-1/2 h-px bg-warn" />}
      {screen && selected && !playing && top && up && (
        <>
          <svg className="pointer-events-none absolute inset-0 overflow-visible" width={width} height={height}>
            <polygon points={screen.map((p) => p.join(",")).join(" ")} fill="none" stroke="var(--color-fg)" strokeWidth={1.5} />
            <line x1={top[0]} y1={top[1]} x2={up[0]} y2={up[1]} stroke="var(--color-fg)" strokeWidth={1.5} />
          </svg>
          {screen.map(([x, y], i) => (
            <div
              key={i}
              role="presentation"
              onPointerDown={onHandle("scale", selected.clipId, selected.corners as Pt[])}
              className="absolute rounded-[2px] border border-black/60 bg-fg"
              style={{ left: x - HANDLE / 2, top: y - HANDLE / 2, width: HANDLE, height: HANDLE, cursor: i % 2 === 0 ? "nwse-resize" : "nesw-resize" }}
            />
          ))}
          <div
            role="presentation"
            title="Rotate (Shift snaps to 15°)"
            onPointerDown={onHandle("rotate", selected.clipId, selected.corners as Pt[])}
            className="absolute rounded-full border border-black/60 bg-fg"
            style={{ left: up[0] - 6, top: up[1] - 6, width: 12, height: 12, cursor: "grab" }}
          />
        </>
      )}
    </div>
  );
}
