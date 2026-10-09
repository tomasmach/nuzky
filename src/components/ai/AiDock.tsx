import { useRef } from "react";
import { create } from "zustand";
import { followPointer } from "../../lib/drag";
import { DOCK_DEFAULT, DOCK_LABELS, DOCK_MIN, clampFloat, clampWidth, dockMax, useDock, type DockMode, type Rect } from "../../lib/dock";
import { PANE_MIN, setPane } from "../../lib/layout";
import { Splitter } from "../ui";
import { AiPanel } from "./AiPanel";

const TOP_BAR = 48;
const GAP = 6;
/** How close to a window edge the pointer must be to dock there. */
const EDGE = 64;

/** While the panel is dragged: where it would land. */
const useDrag = create<{ target: DockMode; zone: Rect } | null>(() => null);

/** The place under the pointer, and the area the panel would take there. */
function targetAt(x: number, y: number, ghost: Rect): { target: DockMode; zone: Rect } {
  const w = window.innerWidth;
  const h = window.innerHeight;
  if (dockMax(w) !== null && y > TOP_BAR) {
    const width = clampWidth(useDock.getState().width, w);
    if (x < EDGE) return { target: "left", zone: { x: GAP, y: TOP_BAR, w: width, h: h - TOP_BAR - GAP } };
    if (x > w - EDGE) return { target: "right", zone: { x: w - GAP - width, y: TOP_BAR, w: width, h: h - TOP_BAR - GAP } };
  }
  const slot = document.querySelector<HTMLElement>("[data-dock-slot=inspector]")?.getBoundingClientRect();
  if (slot && x >= slot.left && x <= slot.right && y >= slot.top && y <= slot.bottom) return { target: "inspector", zone: { x: slot.left, y: slot.top, w: slot.width, h: slot.height } };
  return { target: "float", zone: clampFloat(ghost) };
}

/**
 * Starts moving the panel from a pointer press on its header. A press that does not move is a
 * click (`onClick`). While dragging, the place it would land shows; Esc cancels.
 */
export function startMove(e: React.PointerEvent, panel: HTMLElement, onClick?: () => void) {
  if (e.button !== 0) return;
  e.preventDefault();
  const sx = e.clientX;
  const sy = e.clientY;
  const r = panel.getBoundingClientRect();
  const size = clampFloat(useDock.getState().float);
  // The panel keeps the point that was grabbed under the pointer, as far as its floating size allows.
  const grabX = Math.min(sx - r.left, size.w - 24);
  const grabY = Math.min(sy - r.top, 40);
  let moved = false;
  const end = () => {
    useDrag.setState(null, true);
    window.removeEventListener("keydown", onKey, true);
  };
  const onKey = (k: KeyboardEvent) => {
    if (k.key !== "Escape") return;
    k.preventDefault();
    k.stopPropagation();
    stopFollowing();
    end();
  };
  const stopFollowing = followPointer({
    move: (ev) => {
      if (!moved && Math.hypot(ev.clientX - sx, ev.clientY - sy) < 5) return;
      moved = true;
      useDrag.setState(targetAt(ev.clientX, ev.clientY, { x: ev.clientX - grabX, y: ev.clientY - grabY, w: size.w, h: size.h }), true);
    },
    up: () => {
      const drop = useDrag.getState();
      end();
      if (!moved) return onClick?.();
      if (!drop) return;
      useDock.setState(drop.target === "float" ? { mode: "float", float: drop.zone, open: true } : { mode: drop.target, open: true });
    },
    cancel: end,
  });
  window.addEventListener("keydown", onKey, true);
}

export function moveFloatBy(dx: number, dy: number) {
  const f = clampFloat(useDock.getState().float);
  useDock.setState({ float: clampFloat({ ...f, x: f.x + dx, y: f.y + dy }) });
}

/** Where the dragged panel would land: an outline of that area, with the place's name. */
export function DockDragLayer() {
  const drag = useDrag();
  if (!drag) return null;
  const { zone, target } = drag;
  return (
    <div
      aria-hidden
      className="pointer-events-none fixed z-[100] flex items-center justify-center rounded-xl border-2 border-accent bg-accent/[.10]"
      style={{ left: zone.x, top: zone.y, width: zone.w, height: zone.h }}
    >
      <span className="rounded-md bg-black/60 px-2 py-1 text-[12px] font-medium text-fg">{DOCK_LABELS[target]}</span>
    </div>
  );
}

/**
 * A full-height column at the left or right edge, beside the whole editor. In the inspector's place
 * it is the right column as wide as the inspector, which it replaces, and resizing it resizes the
 * inspector; `inspectorMax` is then the most the inspector may take.
 */
export function DockedAiPanel({ side, width, inspectorMax }: { side: "left" | "right"; width: number; inspectorMax?: number }) {
  const panel = useRef<HTMLElement>(null);
  const inspector = inspectorMax !== undefined;
  return (
    <div className={`relative flex shrink-0 flex-col pb-1.5 ${side === "right" ? "pr-1.5" : "pl-1.5"}`} style={{ width: width + GAP }}>
      <AiPanel panelRef={panel} className="min-h-0 flex-1" />
      {/* The 6 px gap between the editor and the column; the panel grows towards the editor. */}
      <Splitter
        what="AI panel"
        axis="x"
        grow={side === "right" ? -1 : 1}
        size={width}
        min={inspector ? PANE_MIN.inspector : DOCK_MIN}
        max={inspector ? inspectorMax : (dockMax(window.innerWidth) ?? DOCK_MIN)}
        onResize={(w) => (inspector ? setPane("inspector", w) : useDock.setState({ width: w }))}
        onReset={() => (inspector ? setPane("inspector", null) : useDock.setState({ width: DOCK_DEFAULT }))}
        className={`absolute bottom-1.5 top-0 ${side === "right" ? "-left-2" : "-right-2"}`}
      />
    </div>
  );
}

type Edges = { l?: true; r?: true; t?: true; b?: true };
const HANDLES: [Edges, string, string][] = [
  [{ l: true }, "left-0 top-3 bottom-3 w-1.5 cursor-ew-resize", ""],
  [{ r: true }, "right-0 top-3 bottom-3 w-1.5 cursor-ew-resize", ""],
  [{ t: true }, "top-0 left-3 right-3 h-1.5 cursor-ns-resize", ""],
  [{ b: true }, "bottom-0 left-3 right-3 h-1.5 cursor-ns-resize", ""],
  [{ l: true, t: true }, "left-0 top-0 h-3 w-3 cursor-nwse-resize", ""],
  [{ r: true, t: true }, "right-0 top-0 h-3 w-3 cursor-nesw-resize", ""],
  [{ l: true, b: true }, "left-0 bottom-0 h-3 w-3 cursor-nesw-resize", ""],
  [{ r: true, b: true }, "right-0 bottom-0 h-3 w-3 cursor-nwse-resize", "Resize AI panel"],
];

function resize(e: React.PointerEvent, edges: Edges, start: Rect) {
  if (e.button !== 0) return;
  e.preventDefault();
  const sx = e.clientX;
  const sy = e.clientY;
  followPointer({
    move: (ev) => {
      const dx = ev.clientX - sx;
      const dy = ev.clientY - sy;
      // A dragged left or top edge moves that edge and keeps the opposite one where it is.
      const w = Math.max(300, start.w + (edges.r ? dx : edges.l ? -dx : 0));
      const h = Math.max(320, start.h + (edges.b ? dy : edges.t ? -dy : 0));
      const x = edges.l ? start.x + start.w - w : start.x;
      const y = edges.t ? start.y + start.h - h : start.y;
      useDock.setState({ float: clampFloat({ x, y, w, h }) });
    },
  });
}

/** Floats over the editor wherever it was dropped; its edges and corners resize it. */
export function FloatingAiPanel({ rect }: { rect: Rect }) {
  const panel = useRef<HTMLElement>(null);
  return (
    <div className="fixed z-[80] flex rounded-xl shadow-[0_18px_50px_rgb(0_0_0/.55)]" style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}>
      <AiPanel panelRef={panel} className="min-h-0 flex-1 outline-white/[.12]" />
      {HANDLES.map(([edges, cls, label]) => (
        <div
          key={cls}
          onPointerDown={(e) => resize(e, edges, rect)}
          {...(label
            ? {
                role: "separator",
                "aria-label": label,
                tabIndex: 0,
                title: "Drag to resize. With the keyboard, ←/→ change the width and ↑/↓ the height.",
                onKeyDown: (e: React.KeyboardEvent) => {
                  if (!e.key.startsWith("Arrow")) return;
                  e.preventDefault();
                  e.stopPropagation();
                  const step = e.shiftKey ? 96 : 24;
                  const dw = e.key === "ArrowRight" ? step : e.key === "ArrowLeft" ? -step : 0;
                  const dh = e.key === "ArrowDown" ? step : e.key === "ArrowUp" ? -step : 0;
                  useDock.setState({ float: clampFloat({ ...rect, w: rect.w + dw, h: rect.h + dh }) });
                },
              }
            : {})}
          className={`absolute z-10 ${cls} focus-visible:rounded-sm`}
        />
      ))}
    </div>
  );
}
