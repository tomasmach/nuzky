import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
import { Copy, Magnet, Maximize2, PanelLeftClose, PanelRightClose, Scissors, Trash2, ZoomIn, ZoomOut } from "lucide-react";
import { MAIN_TRACK, allClips, contentEnd, deleteSelection, deleteSide, displayTracks, duplicateSelection, editClip, findClip, mainClips, projectDuration, splitAtPlayhead, splitTargets, useAiLocked, useEditor } from "../../lib/store";
import { US, formatDuration, formatTime } from "../../lib/time";
import type { Clip, EditCmd, Project, Track } from "../../lib/types";
import { candidateReason, useCandidates, useCover } from "../../lib/cover";
import { setDropResolver } from "../panel/assets";
import { IconButton, RangeInput } from "../ui";
import { ClipMenu, type MenuAt } from "./ClipMenu";
import { ClipView } from "./ClipView";
import { CutMarkers } from "./CutMarkers";
import { TrackHeader } from "./TrackHeader";
import { dragFade, dragResult, isFade, useTimelineGestures, type Drag } from "./useTimelineGestures";

const HEADER_W = 140;
const RULER_H = 28;
/** Space between lanes. It belongs to the rows, so the track header column runs unbroken. */
const ROW_GAP = 4;
const END_TIP = "The video ends here. Sound after this point is not exported.";

function rowHeight(track: Track) {
  if (track.id === MAIN_TRACK) return 64;
  if (track.kind === "video") return 50;
  if (track.kind === "audio") return 46;
  return 34;
}

type Timing = { startUs: number; durationUs: number };

/** Where the engine puts a clip moved or added to the magnetic main track: before the first other clip whose middle is later. */
function mainInsertIndex(others: Clip[], startUs: number) {
  const i = others.findIndex((c) => c.startUs + c.durationUs / 2 > startUs);
  return i < 0 ? others.length : i;
}

/**
 * The main track as the drag in progress would leave it, so neighbours move live: a moved clip
 * opens a slot where it would go in (`slotUs`, null when it leaves the track) and the gap it left
 * closes; a trim keeps the clip's start and moves everything after it by the change in length.
 */
function mainLayout(project: Project, d: Drag | null, minUs: number): { timing: Map<string, Timing>; slotUs: number | null } | null {
  if (!d?.moved) return null;
  const main = mainClips(project);
  const timing = new Map<string, Timing>();
  const r = dragResult(d, minUs);
  if (d.mode === "move") {
    const dest = d.target === undefined ? d.trackId : d.target;
    if (d.trackId !== MAIN_TRACK && dest !== MAIN_TRACK) return null;
    const others = main.filter((c) => c.id !== d.clip.id);
    const at = dest === MAIN_TRACK ? mainInsertIndex(others, r.startUs) : -1;
    let t = 0;
    let slotUs: number | null = null;
    others.forEach((c, i) => {
      if (i === at) {
        slotUs = t;
        t += d.clip.durationUs;
      }
      timing.set(c.id, { startUs: t, durationUs: c.durationUs });
      t += c.durationUs;
    });
    if (at === others.length) slotUs = t;
    return { timing, slotUs };
  }
  if (d.trackId !== MAIN_TRACK) return null;
  const shift = r.durationUs - d.clip.durationUs;
  for (const c of main)
    timing.set(c.id, c.id === d.clip.id ? { startUs: c.startUs, durationUs: r.durationUs } : { startUs: c.startUs > d.clip.startUs ? c.startUs + shift : c.startUs, durationUs: c.durationUs });
  return { timing, slotUs: null };
}

/** How far a time is from a clip: 0 inside it. */
const distance = (c: Clip, t: number) => (t < c.startUs ? c.startUs - t : Math.max(0, t - c.startUs - c.durationUs));

/**
 * The clip an arrow key moves keyboard focus to: ←/→ the previous or next clip on the same track,
 * ↑/↓ the clip nearest in time on the track above or below that has clips. Null at an edge.
 */
function neighbour(tracks: Track[], track: Track, from: Clip, key: string): Clip | null {
  if (key === "ArrowLeft" || key === "ArrowRight") {
    const clips = [...track.clips].sort((a, b) => a.startUs - b.startUs);
    return clips[clips.findIndex((c) => c.id === from.id) + (key === "ArrowLeft" ? -1 : 1)] ?? null;
  }
  const rows = tracks.filter((t) => t.clips.length > 0);
  const next = rows[rows.findIndex((t) => t.id === track.id) + (key === "ArrowUp" ? -1 : 1)];
  const mid = from.startUs + from.durationUs / 2;
  return next ? next.clips.reduce((best, c) => (distance(c, mid) < distance(best, mid) ? c : best)) : null;
}

/**
 * Alt+←/→ moves a clip by a frame (Shift: a second), stopping at its neighbours so it never jumps to
 * a new track; on the magnetic main track it trades places with the clip beside it. A burst of
 * presses is one undo step.
 */
function nudge(clipId: string, dir: -1 | 1, second: boolean) {
  editClip(
    clipId,
    (c, track, project): EditCmd | null => {
      if (track.id === MAIN_TRACK) {
        const main = [...track.clips].sort((a, b) => a.startUs - b.startUs);
        const other = main[main.findIndex((m) => m.id === c.id) + dir];
        if (!other) return null;
        // The engine puts a moved clip before the first other clip whose middle is later.
        return { type: "moveClip", clipId: c.id, trackId: MAIN_TRACK, startUs: dir < 0 ? other.startUs : Math.round(other.startUs + other.durationUs / 2) };
      }
      const others = track.clips.filter((o) => o.id !== c.id);
      const lo = Math.max(0, ...others.filter((o) => o.startUs + o.durationUs <= c.startUs).map((o) => o.startUs + o.durationUs));
      const hi = Math.min(Number.MAX_SAFE_INTEGER, ...others.filter((o) => o.startUs >= c.startUs + c.durationUs).map((o) => o.startUs)) - c.durationUs;
      const step = second ? US : US / project.canvas.fps;
      const startUs = Math.round(Math.max(lo, Math.min(hi, c.startUs + dir * step)));
      return startUs === c.startUs ? null : { type: "moveClip", clipId: c.id, trackId: track.id, startUs };
    },
    `nudge:${clipId}`,
  );
}

function tickStep(zoom: number): number {
  for (const s of [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600]) if (s * zoom >= 72) return s;
  return 1200;
}

const VISIBLE_STEP = 400;

/** The playhead line, the one part of the timeline that follows playback frame by frame; it keeps itself in view while playing. */
function Playhead({ zoom, scroller }: { zoom: number; scroller: RefObject<HTMLDivElement | null> }) {
  const timeUs = useEditor((s) => s.timeUs);
  const playing = useEditor((s) => s.playing);
  useEffect(() => {
    const el = scroller.current;
    if (!el || !playing) return;
    const x = (timeUs / US) * zoom;
    const lane = el.clientWidth - HEADER_W;
    if (x > el.scrollLeft + lane - 24 || x < el.scrollLeft) el.scrollLeft = Math.max(0, x - 48);
  }, [timeUs, playing, zoom, scroller]);
  // Above the ruler, below the track headers. The dark edges keep it visible on bright footage; no blur, it moves every frame.
  // It moves by transform on its own layer, so playback never repaints the clips under it.
  return (
    <div className="pointer-events-none absolute bottom-0 top-0 z-[45] will-change-transform" style={{ left: HEADER_W, transform: `translateX(${(timeUs / US) * zoom}px)` }}>
      <div className="absolute -left-[0.75px] bottom-0 top-2 w-[1.5px] bg-white shadow-[0_0_0_0.5px_rgb(0_0_0/.4)]" />
      <div className="absolute -left-1.5 top-0.5 h-3.5 w-3 rounded-[3px_3px_6px_6px] bg-white shadow-[0_0_0_0.5px_rgb(0_0_0/.45)]" />
    </div>
  );
}

export function Timeline({ height }: { height: number }) {
  const snap0 = useEditor((s) => s.snap);
  const project = snap0?.project;
  const selection = useEditor((s) => s.selection);
  const cut = useEditor((s) => s.cut);
  // A boolean, so playback re-renders the timeline only when it changes.
  const canSplit = useEditor((s) => (s.snap ? splitTargets(s.snap.project, s.selection, s.timeUs).length > 0 : false));
  const zoom = useEditor((s) => s.zoom);
  // Media dragged from the panel, or files dragged in from the desktop.
  const assetDrag = useEditor((s) => s.assetDrag ?? s.fileDrag);
  const locked = useAiLocked();
  // In the cover editor the timeline chooses the cover's frame: clicks and drags move the playhead, which is the frame.
  const cover = useCover((s) => s.open);
  const candidates = useCandidates();
  const { select, setZoom } = useEditor.getState();
  const scroller = useRef<HTMLDivElement>(null);
  const rows = useRef(new Map<string, HTMLDivElement>());
  const [snapping, setSnapping] = useState(true);
  const [view, setView] = useState({ left: 0, width: 1000 });
  const [menu, setMenu] = useState<MenuAt | null>(null);
  /** The clip keyboard focus was on last; it stays the timeline's tab stop. */
  const [focusId, setFocusId] = useState<string | null>(null);
  /** The focused clip, so focus can move on when it is deleted; null once focus left the clips. */
  const focusPlace = useRef<string | null>(null);
  /** The project as last shown, where a deleted clip's neighbours are found. */
  const shown = useRef(project);
  const closeMenu = useCallback(() => setMenu(null), []);

  const tracks = useMemo(() => (project ? displayTracks(project) : []), [project]);
  const duration = project ? contentEnd(project) : 0;
  const fps = project?.canvas.fps ?? 30;
  const minUs = Math.ceil(US / fps);
  const laneWidth = Math.max(view.width - HEADER_W, (duration / US + 30) * zoom);
  // Clips get the window they draw filmstrip tiles in. It moves in steps, with at least 200 px of margin on
  // each side, so scrolling re-renders the clips once per step instead of on every scroll event.
  const windowFrom = Math.floor(view.left / VISIBLE_STEP) * VISIBLE_STEP;
  const visible = useMemo<[number, number]>(() => [windowFrom - 200, windowFrom + view.width + VISIBLE_STEP + 200], [windowFrom, view.width]);

  const timeAt = useCallback(
    (clientX: number) => {
      const el = scroller.current!;
      const rect = el.getBoundingClientRect();
      return Math.max(0, ((clientX - rect.left + el.scrollLeft - HEADER_W) / zoom) * US);
    },
    [zoom],
  );

  // Media dragged from the media panel lands here.
  useEffect(() => {
    setDropResolver((x, y) => {
      const el = scroller.current;
      if (!el) return null;
      const r = el.getBoundingClientRect();
      if (x < r.left || x > r.right || y < r.top || y > r.bottom) return null;
      let trackId: string | null = null;
      for (const [id, row] of rows.current) {
        const rr = row.getBoundingClientRect();
        if (y >= rr.top && y <= rr.bottom) trackId = id;
      }
      return { trackId, startUs: Math.round(timeAt(x)) };
    });
    return () => setDropResolver(null);
  }, [timeAt]);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const update = () => setView({ left: el.scrollLeft, width: el.clientWidth });
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Zooming keeps one moment where it is on screen: the one under the pointer for Ctrl + wheel,
  // otherwise the playhead, which comes to the middle when it was out of view.
  const zoomAnchor = useRef<{ t: number; px: number } | null>(null);
  const shownZoom = useRef(zoom);
  useLayoutEffect(() => {
    const el = scroller.current;
    const before = shownZoom.current;
    shownZoom.current = zoom;
    if (!el || before === zoom) return;
    const lane = el.clientWidth - HEADER_W;
    let anchor = zoomAnchor.current;
    zoomAnchor.current = null;
    if (!anchor) {
      const t = useEditor.getState().timeUs / US;
      const px = t * before - el.scrollLeft;
      anchor = { t, px: px >= 0 && px <= lane ? px : lane / 2 };
    }
    el.scrollLeft = Math.max(0, anchor.t * zoom - anchor.px);
  }, [zoom]);

  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      const z = useEditor.getState().zoom;
      const px = e.clientX - el.getBoundingClientRect().left - HEADER_W;
      zoomAnchor.current = { t: (el.scrollLeft + px) / z, px };
      setZoom(z * (e.deltaY < 0 ? 1.15 : 1 / 1.15));
      // Already at the limit: nothing moves, and the next zoom anchors on the playhead again.
      if (useEditor.getState().zoom === z) zoomAnchor.current = null;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [setZoom]);

  const { drag: gesture, startClipDrag, startScrub } = useTimelineGestures({ project, zoom, snapping, minUs, rows, timeAt });
  // A fade drag changes only the clip's fades; the ghost, the slot and the snap guide belong to moves and trims.
  const drag = gesture && !isFade(gesture.mode) ? gesture : null;
  const fading = gesture?.moved && isFade(gesture.mode) ? gesture : null;

  // When the focused clip is deleted, focus and select the clip that followed it on its track, else
  // the one before it, so the keyboard keeps its place. Neighbours come from the order shown before
  // the deletion, so a clip moved meanwhile counts where it was.
  useLayoutEffect(() => {
    const before = shown.current;
    shown.current = project;
    const id = focusPlace.current;
    if (!project || !before || !id || findClip(project, id)) return;
    focusPlace.current = null;
    if (document.activeElement && document.activeElement !== document.body) return;
    const clips = [...(findClip(before, id)?.track.clips ?? [])].sort((a, b) => a.startUs - b.startUs);
    const at = clips.findIndex((c) => c.id === id);
    const next = [...clips.slice(at + 1), ...clips.slice(0, at).reverse()].find((c) => findClip(project, c.id));
    if (!next) return;
    select([next.id]);
    scroller.current?.querySelector<HTMLElement>(`[data-clip-id="${next.id}"]`)?.focus({ preventScroll: true });
  }, [project, select]);

  const openMenu = useCallback((e: React.MouseEvent, clip: Clip) => {
    e.preventDefault();
    if (!useEditor.getState().selection.includes(clip.id)) useEditor.getState().select([clip.id]);
    setMenu({ clipId: clip.id, x: e.clientX, y: e.clientY });
  }, []);

  if (!project) return <section className="pane shrink-0" style={{ height }} />;

  // The video ends at its last picture or text; music running past it is cut on export.
  const videoEnd = projectDuration(project);
  const endX = (videoEnd / US) * zoom;
  const pastEnd = videoEnd > 0 && duration > videoEnd;
  const step = tickStep(zoom);
  const firstTick = Math.floor(Math.max(0, view.left - 100) / zoom / step) * step;
  const lastTick = (view.left + view.width + 100) / zoom;
  const ticks: number[] = [];
  for (let t = firstTick; t <= lastTick; t += step) ticks.push(Math.round(t * 1000) / 1000);

  const layout = mainLayout(project, drag, minUs);
  const fadeUs = fading ? dragFade(fading) : 0;
  const fadeContent = fading?.clip.content.type === "media" ? fading.clip.content : null;
  const fades = fading && fadeContent ? { inUs: fading.mode === "fadeIn" ? fadeUs : fadeContent.fadeInUs, outUs: fading.mode === "fadeOut" ? fadeUs : fadeContent.fadeOutUs } : undefined;
  // A trimmed main-track clip shows where it ends up; elsewhere the ghost follows the pointer.
  const ghostTiming = drag?.moved ? ((drag.mode !== "move" && layout?.timing.get(drag.clip.id)) || dragResult(drag, minUs)) : null;
  const ghostTrackId = drag?.moved ? (drag.target === undefined ? drag.trackId : drag.target) : null;
  // Media dropped on the magnetic main track goes in at a cut, so the line shows that cut.
  const dropTime =
    assetDrag && scroller.current
      ? (() => {
          const r = scroller.current.getBoundingClientRect();
          if (assetDrag.x < r.left || assetDrag.x > r.right || assetDrag.y < r.top || assetDrag.y > r.bottom) return null;
          const t = timeAt(assetDrag.x);
          const mainRow = rows.current.get(MAIN_TRACK)?.getBoundingClientRect();
          if (!mainRow || assetDrag.y < mainRow.top || assetDrag.y > mainRow.bottom) return t;
          const main = mainClips(project);
          const i = mainInsertIndex(main, t);
          return i < main.length ? main[i].startUs : main.reduce((end, c) => Math.max(end, c.startUs + c.durationUs), 0);
        })()
      : null;
  const emptyTimeline = allClips(project).length === 0;
  // One tab stop for all clips: the last focused, else the first selected, else the first main-track clip.
  const known = (id: string | null | undefined) => !!id && allClips(project).some((c) => c.id === id);
  const tabStop = known(focusId) ? focusId : known(selection[0]) ? selection[0] : (mainClips(project)[0]?.id ?? tracks.find((t) => t.clips.length > 0)?.clips[0]?.id ?? null);

  /** Focuses a clip and scrolls it into view, clear of the sticky ruler and track headers. */
  const focusClip = (id: string) => {
    const box = scroller.current;
    const el = box?.querySelector<HTMLElement>(`[data-clip-id="${id}"]`);
    if (!box || !el) return;
    el.focus({ preventScroll: true });
    const view = box.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    const [left, right] = [view.left + HEADER_W, view.left + box.clientWidth];
    const [top, bottom] = [view.top + RULER_H, view.top + box.clientHeight];
    if (r.left < left) box.scrollLeft -= left - r.left + 24;
    else if (r.right > right) box.scrollLeft += Math.min(r.left - left - 24, r.right - right + 24);
    if (r.top < top) box.scrollTop -= top - r.top;
    else if (r.bottom > bottom) box.scrollTop += r.bottom - bottom;
  };

  // Arrows move between clips and select the one they reach, so Delete, S, Q, W and Ctrl+D act on
  // the clip with focus. Ctrl+arrows move focus only, to add clips with Shift+Enter; Alt+arrows
  // nudge; the Menu key or Shift+F10 opens the clip menu. The keys stop here, so the playhead does
  // not also step.
  const onClipKey = (e: React.KeyboardEvent) => {
    if (cover) return;
    const id = (e.target as HTMLElement).getAttribute?.("data-clip-id");
    const found = id ? tracks.flatMap((t) => t.clips.map((c) => [t, c] as const)).find(([, c]) => c.id === id) : undefined;
    if (!found || e.metaKey) return;
    const [track, clip] = found;
    const arrow = e.key === "ArrowLeft" || e.key === "ArrowRight" || e.key === "ArrowUp" || e.key === "ArrowDown";
    if ((e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      e.stopPropagation();
      if (!useEditor.getState().selection.includes(clip.id)) select([clip.id]);
      const r = (e.target as HTMLElement).getBoundingClientRect();
      setMenu({ clipId: clip.id, x: r.left + 8, y: r.bottom - 4 });
    } else if (e.ctrlKey) {
      if (!arrow || e.altKey || e.shiftKey) return;
      e.preventDefault();
      e.stopPropagation();
      const next = neighbour(tracks, track, clip, e.key);
      if (next) focusClip(next.id);
    } else if (arrow && e.altKey) {
      if (e.key === "ArrowUp" || e.key === "ArrowDown") return;
      e.preventDefault();
      e.stopPropagation();
      nudge(clip.id, e.key === "ArrowLeft" ? -1 : 1, e.shiftKey);
    } else if (arrow && !e.shiftKey) {
      e.preventDefault();
      e.stopPropagation();
      const next = neighbour(tracks, track, clip, e.key);
      if (next) {
        focusClip(next.id);
        select([next.id]);
      }
    } else if (e.key === "Enter") {
      e.preventDefault();
      const sel = useEditor.getState().selection;
      if (e.shiftKey) select(sel.includes(clip.id) ? sel.filter((s) => s !== clip.id) : [...sel, clip.id]);
      else select([clip.id]);
    }
  };
  const dragKind = drag ? project.tracks.find((t) => t.id === drag.trackId)?.kind : null;
  const deleteLabel = cut ? "Delete transition (Delete)" : selection.length ? "Delete selection (Delete)" : "Delete: select a clip first";
  // While the AI edits, the editing buttons say so instead of what they would need.
  const lockedLabel = (action: string) => `${action}: the AI is editing`;

  return (
    <section className="pane flex shrink-0 flex-col overflow-hidden" style={{ height }} aria-label="Timeline">
      <div className="flex h-11 shrink-0 items-center gap-1.5 px-2.5">
        {cover ? (
          <p className="truncate pl-1.5 text-[12px] text-muted">Click or drag on the timeline to choose the cover's frame.</p>
        ) : (
          <div className="bar flex items-center rounded-full">
            <IconButton round label={locked ? lockedLabel("Split") : canSplit ? "Split at playhead (S)" : "Split: move the playhead over a clip"} disabled={locked || !canSplit} onClick={splitAtPlayhead}>
              <Scissors size={16} />
            </IconButton>
            <IconButton
              round
              label={locked ? lockedLabel("Delete left") : canSplit ? "Delete left of playhead (Q)" : "Delete left: move the playhead over a clip"}
              disabled={locked || !canSplit}
              onClick={() => deleteSide("left")}
            >
              <PanelLeftClose size={16} />
            </IconButton>
            <IconButton
              round
              label={locked ? lockedLabel("Delete right") : canSplit ? "Delete right of playhead (W)" : "Delete right: move the playhead over a clip"}
              disabled={locked || !canSplit}
              onClick={() => deleteSide("right")}
            >
              <PanelRightClose size={16} />
            </IconButton>
            <IconButton round label={locked ? lockedLabel("Delete") : deleteLabel} disabled={locked || (selection.length === 0 && !cut)} onClick={deleteSelection}>
              <Trash2 size={16} />
            </IconButton>
            <IconButton
              round
              label={locked ? lockedLabel("Duplicate") : selection.length ? "Duplicate (Ctrl+D)" : "Duplicate: select a clip first"}
              disabled={locked || selection.length === 0}
              onClick={duplicateSelection}
            >
              <Copy size={15} />
            </IconButton>
            <span aria-hidden className="mx-1 h-4 w-px bg-white/[.12]" />
            <IconButton round label={snapping ? "Snapping on" : "Snapping off"} active={snapping} onClick={() => setSnapping(!snapping)}>
              <Magnet size={16} />
            </IconButton>
          </div>
        )}
        <div className="flex-1" />
        <IconButton round label="Zoom out (-)" onClick={() => setZoom(zoom / 1.3)}>
          <ZoomOut size={16} />
        </IconButton>
        <RangeInput
          label="Timeline zoom"
          min={Math.log(4)}
          max={Math.log(600)}
          step={0.01}
          value={Math.log(zoom)}
          valueText={`${Math.round(zoom)} pixels per second`}
          onChange={(v) => setZoom(Math.exp(v))}
          className="w-24"
        />
        <IconButton round label="Zoom in (+)" onClick={() => setZoom(zoom * 1.3)}>
          <ZoomIn size={16} />
        </IconButton>
        <IconButton
          round
          label="Fit timeline to view"
          disabled={duration === 0}
          onClick={() => {
            const w = (scroller.current?.clientWidth ?? 800) - HEADER_W - 40;
            zoomAnchor.current = { t: 0, px: 0 };
            setZoom(w / Math.max(1, duration / US));
            if (useEditor.getState().zoom === zoom && scroller.current) {
              zoomAnchor.current = null;
              scroller.current.scrollLeft = 0;
            }
          }}
        >
          <Maximize2 size={15} />
        </IconButton>
      </div>

      {/* Inset by 1 px, so the pane's hairline stays visible beside the opaque track headers. */}
      <div
        ref={scroller}
        className="relative mx-px mb-px min-h-0 flex-1 overflow-auto"
        onScroll={(e) => setView({ left: e.currentTarget.scrollLeft, width: e.currentTarget.clientWidth })}
      >
        {/*
          Layers, bottom to top: lanes and clips, the ruler (40), the playhead (45), the track header
          column (47) and the corner above it (48). The corner is not part of the ruler: the ruler is
          its own stacking context, and the playhead has to pass over the ruler but under the corner.
        */}
        <div className="relative flex flex-col" style={{ width: HEADER_W + laneWidth, minHeight: "100%" }}>
          <div className="sticky left-0 top-0 z-[48] shrink-0 border-b border-r border-white/[.07] bg-panel" style={{ width: HEADER_W, height: RULER_H, marginBottom: -RULER_H }} />
          {/* Ruler */}
          <div className="sticky top-0 z-40 flex shrink-0" style={{ height: RULER_H }}>
            <div className="shrink-0" style={{ width: HEADER_W }} />
            <div className="relative flex-1 cursor-pointer border-b border-white/[.07] bg-panel" onPointerDown={startScrub}>
              {ticks.map((t) => (
                <div key={t} className="absolute top-0 h-full" style={{ left: t * zoom }}>
                  <span className="tabular absolute left-1 top-[5px] text-[11px] leading-[13px] text-muted">{formatTime(t * US, step < 1)}</span>
                  <div className="absolute bottom-0 h-2 w-px bg-subtle" />
                  {[1, 2, 3, 4].map((i) => (
                    <div key={i} className="absolute bottom-0 h-1 w-px bg-subtle/60" style={{ left: (i * step * zoom) / 5 }} />
                  ))}
                </div>
              ))}
              {pastEnd && (
                <div className="absolute inset-y-0 w-2 -translate-x-1/2" style={{ left: endX }} title={END_TIP}>
                  <div className="mx-auto h-full w-0 border-l border-dashed border-muted" />
                </div>
              )}
              {/* The frames Pick for me found, as dots along the bottom of the ruler. */}
              {cover &&
                candidates.map((c) => (
                  <span
                    key={c.timeUs}
                    className="absolute bottom-0.5 h-1.5 w-1.5 -translate-x-1/2 rounded-full bg-fg/60 shadow-[0_0_0_1px_rgb(0_0_0/.5)]"
                    style={{ left: (c.timeUs / US) * zoom }}
                    title={`${formatTime(c.timeUs)} · ${Math.round(c.score * 100)} · ${candidateReason(c, cover)}`}
                  />
                ))}
            </div>
          </div>

          {/* Tracks */}
          <div
            className="relative flex flex-1 flex-col"
            onKeyDown={onClipKey}
            onFocus={(e) => {
              const id = (e.target as HTMLElement).getAttribute?.("data-clip-id");
              if (!id) return;
              setFocusId(id);
              focusPlace.current = id;
            }}
            // A removed element fires no blur, so a deleted clip keeps its place for the effect below.
            onBlur={(e) => e.target.isConnected && (focusPlace.current = null)}
          >
            {drag?.moved && drag.target === null && dragKind !== "audio" && <div className="ml-[140px] h-0.5 rounded-full bg-accent" title="Drop to create a new track" />}
            {tracks.map((track) => {
              const h = rowHeight(track);
              const isMain = track.id === MAIN_TRACK;
              return (
                <div key={track.id} className="flex shrink-0" style={{ height: h + ROW_GAP }}>
                  <TrackHeader track={track} width={HEADER_W} locked={locked} />
                  <div
                    ref={(el) => {
                      if (el) rows.current.set(track.id, el);
                      else rows.current.delete(track.id);
                    }}
                    role="listbox"
                    aria-label={`${track.name || track.kind} track`}
                    aria-multiselectable
                    className={`relative flex-1 ${isMain ? "bg-white/[0.035]" : "bg-white/[0.015]"} ${assetDrag && dropTime !== null ? "outline-1 outline-dashed outline-white/20" : ""}`}
                    style={{ marginBlock: ROW_GAP / 2 }}
                    onPointerDown={(e) => {
                      if (e.target === e.currentTarget) {
                        select([]);
                        startScrub(e);
                      }
                    }}
                  >
                    {isMain && emptyTimeline && (
                      <div className="pointer-events-none absolute inset-1 flex items-center justify-center rounded-[7px] border border-dashed border-white/15 text-[12px] text-muted">
                        Drag media here, or press + on a media item
                      </div>
                    )}
                    {track.clips.map((clip) => {
                      if (drag?.moved && drag.clip.id === clip.id) {
                        // A trimmed main-track clip is drawn by its ghost in place; a moved one leaves its old spot.
                        if (isMain && layout) return null;
                        return (
                          <div
                            key={clip.id}
                            className="absolute top-1 bottom-1 rounded-[7px] border border-dashed border-muted/50"
                            style={{ left: (clip.startUs / US) * zoom + 1, width: (clip.durationUs / US) * zoom - 2 }}
                          />
                        );
                      }
                      return (
                        <ClipView
                          key={clip.id}
                          clip={clip}
                          track={track}
                          project={project}
                          zoom={zoom}
                          height={h}
                          visible={visible}
                          selected={selection.includes(clip.id)}
                          locked={locked}
                          tabbable={!cover && clip.id === tabStop}
                          timing={isMain ? layout?.timing.get(clip.id) : undefined}
                          fades={fading?.clip.id === clip.id ? fades : undefined}
                          onPointerDown={startClipDrag}
                          onContextMenu={openMenu}
                        />
                      );
                    })}
                    {/* Where a moved clip would go in on the main track: its slot and an insertion bar. */}
                    {isMain && drag?.moved && layout?.slotUs != null && (
                      <>
                        <div
                          className="pointer-events-none absolute top-1 bottom-1 rounded-[7px] border border-dashed border-muted/50"
                          style={{ left: (layout.slotUs / US) * zoom + 1, width: (drag.clip.durationUs / US) * zoom - 2 }}
                        />
                        <div className="pointer-events-none absolute -top-0.5 -bottom-0.5 z-20 w-0.5 -translate-x-1/2 rounded-full bg-accent" style={{ left: (layout.slotUs / US) * zoom }} />
                      </>
                    )}
                    {isMain && !drag?.moved && <CutMarkers project={project} zoom={zoom} locked={locked} />}
                    {drag?.moved && ghostTiming && ghostTrackId === track.id && (
                      <ClipView clip={drag.clip} track={track} project={project} zoom={zoom} height={h} visible={visible} selected ghost timing={ghostTiming} />
                    )}
                  </div>
                </div>
              );
            })}
            {drag?.moved && drag.target === null && dragKind === "audio" && <div className="ml-[140px] h-0.5 rounded-full bg-accent" />}
            {/* The header column runs on below the last track, so nothing scrolled under it shows through. */}
            <div className="flex flex-1">
              <div className="sticky left-0 z-[47] shrink-0 border-r border-white/[.07] bg-panel" style={{ width: HEADER_W }} />
            </div>
          </div>

          {/* In the cover editor the lanes only choose the frame; clips cannot be selected, moved or trimmed. */}
          {cover && <div className="absolute bottom-0 right-0 z-[41] cursor-pointer" style={{ left: HEADER_W, top: RULER_H }} onPointerDown={startScrub} data-testid="cover-scrub" />}

          {/* Ghost for a drag onto a new track follows the pointer row-less. */}
          {drag?.moved && drag.target === null && ghostTiming && (
            <div className="pointer-events-none absolute z-30" style={{ left: HEADER_W, top: RULER_H, height: 44, width: laneWidth }}>
              <ClipView
                clip={drag.clip}
                track={project.tracks.find((t) => t.id === drag.trackId)!}
                project={project}
                zoom={zoom}
                height={44}
                visible={visible}
                selected
                ghost
                timing={ghostTiming}
              />
            </div>
          )}

          {/* Past the video's end: dimmed above the clips, below the track headers. */}
          {pastEnd && (
            <div
              className="pointer-events-none absolute bottom-0 z-[15] border-l border-dashed border-muted bg-black/30"
              style={{ left: HEADER_W + endX, top: RULER_H, width: laneWidth - endX }}
            />
          )}

          {/* Snap guide */}
          {drag?.moved && drag.snapUs !== null && (
            <div className="pointer-events-none absolute bottom-0 top-0 z-30 w-px bg-warn shadow-[0_0_0_1px_rgba(0,0,0,0.55)]" style={{ left: HEADER_W + (drag.snapUs / US) * zoom }} />
          )}

          {/* Drop position for media dragged from the panel */}
          {dropTime !== null && <div className="pointer-events-none absolute bottom-0 top-0 z-30 w-0.5 bg-accent" style={{ left: HEADER_W + (dropTime / US) * zoom }} />}

          <Playhead zoom={zoom} scroller={scroller} />

          {drag?.moved && ghostTiming && (
            // Pinned to the edge being dragged: the start for a move or left trim, the end for a right trim.
            <div
              className={`tabular pointer-events-none absolute z-50 rounded-md bg-black/80 px-1.5 py-0.5 text-[11px] font-medium text-fg ${drag.mode === "trimR" ? "-translate-x-full" : ""}`}
              style={{ left: HEADER_W + ((drag.mode === "trimR" ? ghostTiming.startUs + ghostTiming.durationUs : ghostTiming.startUs) / US) * zoom, top: 2 }}
            >
              {drag.mode === "move" ? formatTime(layout?.slotUs ?? ghostTiming.startUs) : formatDuration(ghostTiming.durationUs)}
            </div>
          )}
          {fading && (
            // Pinned to the end of the fade, like a trim's tooltip to its edge.
            <div
              className={`tabular pointer-events-none absolute z-50 rounded-md bg-black/80 px-1.5 py-0.5 text-[11px] font-medium text-fg ${fading.mode === "fadeOut" ? "-translate-x-full" : ""}`}
              style={{ left: HEADER_W + ((fading.mode === "fadeIn" ? fading.clip.startUs + fadeUs : fading.clip.startUs + fading.clip.durationUs - fadeUs) / US) * zoom, top: 2 }}
            >
              {fading.mode === "fadeIn" ? "Fade in" : "Fade out"} {formatDuration(fadeUs)}
            </div>
          )}
        </div>
      </div>
      {menu && <ClipMenu at={menu} onClose={closeMenu} />}
    </section>
  );
}
