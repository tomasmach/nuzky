import { create } from "zustand";

/** The editor's resizable panes: the library and inspector widths and the timeline height. */
export type Pane = "library" | "inspector" | "timeline";

const KEYS: Record<Pane, string> = { library: "nuzky.libraryWidth", inspector: "nuzky.inspectorWidth", timeline: "nuzky.timelineHeight" };
/** The library is at least 360 px: at 340 px its tab labels touch. */
export const PANE_MIN: Record<Pane, number> = { library: 360, inspector: 300, timeline: 160 };
/** The share of the window a pane takes until someone resizes it, close to CapCut's. */
const SHARE: Record<Pane, number> = { library: 0.28, inspector: 0.28, timeline: 0.35 };
/** The narrowest preview whose transport still shows the current time in full; it keeps 300 px of height. */
export const PREVIEW_MIN = 340;
const PREVIEW_MIN_H = 300;
const TOP_BAR = 48;
const GAP = 6;

const load = (pane: Pane) => {
  const v = Number(localStorage.getItem(KEYS[pane]));
  return v > 0 ? v : null;
};

/** What the user dragged each pane to; null follows the window. */
export const usePanes = create<Record<Pane, number | null>>(() => ({ library: load("library"), inspector: load("inspector"), timeline: load("timeline") }));

/** Remembers a size, or with null goes back to following the window. */
export function setPane(pane: Pane, size: number | null) {
  if (size === null) localStorage.removeItem(KEYS[pane]);
  else localStorage.setItem(KEYS[pane], String(size));
  usePanes.setState({ [pane]: size });
}

/**
 * The sizes the panes get in an editor `editorW` wide (the window less a docked AI column) and a
 * window `h` high, and the most each may take now. When the preview would get under 340 px, the
 * inspector gives up space first, then the library; each gets its size back when there is room.
 */
export function paneLayout(saved: Record<Pane, number | null>, editorW: number, h: number) {
  const room = editorW - 4 * GAP - PREVIEW_MIN;
  const wanted = (pane: "library" | "inspector") => Math.max(PANE_MIN[pane], saved[pane] ?? Math.round(editorW * SHARE[pane]));
  const inspector = Math.max(PANE_MIN.inspector, Math.min(wanted("inspector"), room - wanted("library")));
  const library = Math.max(PANE_MIN.library, Math.min(wanted("library"), room - inspector));
  const timelineMax = Math.max(PANE_MIN.timeline, h - TOP_BAR - PREVIEW_MIN_H - 2 * GAP);
  const timeline = Math.min(timelineMax, Math.max(PANE_MIN.timeline, saved.timeline ?? Math.round(h * SHARE.timeline)));
  return {
    library,
    inspector,
    timeline,
    max: { library: Math.max(PANE_MIN.library, room - inspector), inspector: Math.max(PANE_MIN.inspector, room - library), timeline: timelineMax },
  };
}
