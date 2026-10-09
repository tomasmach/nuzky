import { useEffect, useState } from "react";
import { create } from "zustand";

/** Where the AI panel sits: a full-height column at either edge, the inspector's place, or floating. */
export type DockMode = "right" | "left" | "inspector" | "float";
export type Rect = { x: number; y: number; w: number; h: number };

export const DOCK_DEFAULT = 360;
export const DOCK_MIN = 300;
const DOCK_MAX = 640;
const FLOAT_MIN_W = 300;
const FLOAT_MIN_H = 320;
const TOP_BAR = 48;
const GAP = 6;
/** The editor beside a docked panel: library, inspector, a 300 px preview, and the gaps. */
const EDITOR_MIN = 360 + 300 + 300 + 4 * GAP;

type Saved = { open: boolean; mode: DockMode; width: number; float: Rect };
const KEY = "capopen.aiPanel";

const defaultFloat = (): Rect => ({ x: window.innerWidth - GAP - 380, y: TOP_BAR + GAP, w: 380, h: Math.min(600, window.innerHeight - TOP_BAR - 2 * GAP) });

function load(): Saved {
  const fallback: Saved = { open: false, mode: "right", width: DOCK_DEFAULT, float: defaultFloat() };
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "null");
    if (!v || typeof v !== "object") return fallback;
    const f = v.float;
    const float = f && [f.x, f.y, f.w, f.h].every(Number.isFinite) ? f : fallback.float;
    return {
      open: v.open === true,
      mode: ["right", "left", "inspector", "float"].includes(v.mode) ? v.mode : "right",
      width: Number.isFinite(v.width) ? v.width : DOCK_DEFAULT,
      float,
    };
  } catch {
    return fallback;
  }
}

export const useDock = create<Saved>(() => load());
useDock.subscribe((s) => localStorage.setItem(KEY, JSON.stringify({ open: s.open, mode: s.mode, width: s.width, float: s.float })));

export const togglePanel = () => useDock.setState((s) => ({ open: !s.open }));

/** The widest a docked column can be in this window, or null when a column does not fit at all. */
export function dockMax(windowW: number): number | null {
  const room = windowW - EDITOR_MIN - GAP;
  return room < DOCK_MIN ? null : Math.min(DOCK_MAX, room);
}

export const clampWidth = (w: number, windowW: number) => Math.round(Math.max(DOCK_MIN, Math.min(dockMax(windowW) ?? DOCK_MIN, w)));

/** Keeps a floating panel inside the window, at least its minimum size, with its header reachable. */
export function clampFloat(r: Rect, windowW = window.innerWidth, windowH = window.innerHeight): Rect {
  const w = Math.round(Math.max(FLOAT_MIN_W, Math.min(windowW - 2 * GAP, r.w)));
  const h = Math.round(Math.max(FLOAT_MIN_H, Math.min(windowH - 2 * GAP, r.h)));
  const x = Math.round(Math.max(GAP, Math.min(windowW - GAP - w, r.x)));
  const y = Math.round(Math.max(GAP, Math.min(windowH - GAP - h, r.y)));
  return { x, y, w, h };
}

function useWindowSize() {
  const [size, setSize] = useState({ w: window.innerWidth, h: window.innerHeight });
  useEffect(() => {
    const onResize = () => setSize({ w: window.innerWidth, h: window.innerHeight });
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return size;
}

/**
 * Where the panel actually shows in this window. A column that no longer fits beside the editor
 * takes the inspector's place instead; the chosen place comes back once the window is wide enough.
 */
export function useDockLayout() {
  const { open, mode, width, float } = useDock();
  const { w, h } = useWindowSize();
  const sideFits = dockMax(w) !== null;
  const shown: DockMode = (mode === "left" || mode === "right") && !sideFits ? "inspector" : mode;
  return { open, mode: shown, width: shown === "inspector" ? 300 : clampWidth(width, w), float: clampFloat(float, w, h), sideFits };
}

export const DOCK_LABELS: Record<DockMode, string> = {
  right: "Dock right",
  left: "Dock left",
  inspector: "In place of the inspector",
  float: "Floating",
};

export const TOO_NARROW = "The window is too narrow for a column here. Make it wider, or use the inspector's place.";
