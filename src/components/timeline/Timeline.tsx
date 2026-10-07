import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
import { Copy, Magnet, Maximize2, PanelLeftClose, PanelRightClose, Scissors, Trash2, ZoomIn, ZoomOut } from "lucide-react";
import { MAIN_TRACK, allClips, contentEnd, deleteSelection, deleteSide, displayTracks, duplicateSelection, projectDuration, splitAtPlayhead, splitTargets, useAiLocked, useEditor } from "../../lib/store";
import { US, formatDuration, formatTime } from "../../lib/time";
import type { Clip, Track } from "../../lib/types";
import { setDropResolver } from "../panel/assets";
import { IconButton, RangeInput } from "../ui";
import { ClipMenu, type MenuAt } from "./ClipMenu";
import { ClipView } from "./ClipView";
import { CutMarkers } from "./CutMarkers";
import { TrackHeader } from "./TrackHeader";
import { dragResult, useTimelineGestures } from "./useTimelineGestures";

const HEADER_W = 132;
const RULER_H = 28;
const END_TIP = "The video ends here. Sound after this point is not exported.";

function rowHeight(track: Track) {
  if (track.id === MAIN_TRACK) return 64;
  if (track.kind === "video") return 50;
  if (track.kind === "audio") return 46;
  return 34;
}

function tickStep(zoom: number): number {
  for (const s of [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600]) if (s * zoom >= 72) return s;
  return 1200;
}

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
  return (
    <div className="pointer-events-none absolute bottom-0 top-0 z-[45]" style={{ left: HEADER_W + (timeUs / US) * zoom }}>
      <div className="absolute -left-[5px] top-0 h-3 w-[11px] rounded-b-sm bg-fg" />
      <div className="absolute left-0 top-0 h-full w-px bg-fg" />
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
  const { select, setZoom } = useEditor.getState();
  const scroller = useRef<HTMLDivElement>(null);
  const rows = useRef(new Map<string, HTMLDivElement>());
  const [snapping, setSnapping] = useState(true);
  const [view, setView] = useState({ left: 0, width: 1000 });
  const [menu, setMenu] = useState<MenuAt | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);

  const tracks = useMemo(() => (project ? displayTracks(project) : []), [project]);
  const duration = project ? contentEnd(project) : 0;
  const fps = project?.canvas.fps ?? 30;
  const minUs = Math.ceil(US / fps);
  const laneWidth = Math.max(view.width - HEADER_W, (duration / US + 30) * zoom);
  const visible = useMemo<[number, number]>(() => [view.left - 200, view.left + view.width + 200], [view.left, view.width]);

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

  const { drag, startClipDrag, startScrub } = useTimelineGestures({ project, zoom, snapping, minUs, rows, timeAt });

  const openMenu = useCallback((e: React.MouseEvent, clip: Clip) => {
    e.preventDefault();
    if (!useEditor.getState().selection.includes(clip.id)) useEditor.getState().select([clip.id]);
    setMenu({ clipId: clip.id, x: e.clientX, y: e.clientY });
  }, []);

  if (!project) return <section className="shrink-0 border-t border-line bg-panel" style={{ height }} />;

  // The video ends at its last picture or text; music running past it is cut on export.
  const videoEnd = projectDuration(project);
  const endX = (videoEnd / US) * zoom;
  const pastEnd = videoEnd > 0 && duration > videoEnd;
  const step = tickStep(zoom);
  const firstTick = Math.floor(Math.max(0, view.left - 100) / zoom / step) * step;
  const lastTick = (view.left + view.width + 100) / zoom;
  const ticks: number[] = [];
  for (let t = firstTick; t <= lastTick; t += step) ticks.push(Math.round(t * 1000) / 1000);

  const ghostTiming = drag?.moved ? dragResult(drag, minUs) : null;
  const ghostTrackId = drag?.moved ? (drag.target === undefined ? drag.trackId : drag.target) : null;
  const dropTime =
    assetDrag && scroller.current
      ? (() => {
          const r = scroller.current.getBoundingClientRect();
          return assetDrag.x >= r.left && assetDrag.x <= r.right && assetDrag.y >= r.top && assetDrag.y <= r.bottom ? timeAt(assetDrag.x) : null;
        })()
      : null;
  const emptyTimeline = allClips(project).length === 0;
  const dragKind = drag ? project.tracks.find((t) => t.id === drag.trackId)?.kind : null;
  const deleteLabel = cut ? "Delete transition (Delete)" : selection.length ? "Delete selection (Delete)" : "Delete: select a clip first";
  // While the AI edits, the editing buttons say so instead of what they would need.
  const lockedLabel = (action: string) => `${action}: the AI is editing`;

  return (
    <section className="flex shrink-0 flex-col border-t border-line bg-panel" style={{ height }} aria-label="Timeline">
      <div className="flex h-10 shrink-0 items-center gap-1 border-b border-line px-2">
        <IconButton label={locked ? lockedLabel("Split") : canSplit ? "Split at playhead (S)" : "Split: move the playhead over a clip"} disabled={locked || !canSplit} onClick={splitAtPlayhead}>
          <Scissors size={16} />
        </IconButton>
        <IconButton
          label={locked ? lockedLabel("Delete left") : canSplit ? "Delete left of playhead (Q)" : "Delete left: move the playhead over a clip"}
          disabled={locked || !canSplit}
          onClick={() => deleteSide("left")}
        >
          <PanelLeftClose size={16} />
        </IconButton>
        <IconButton
          label={locked ? lockedLabel("Delete right") : canSplit ? "Delete right of playhead (W)" : "Delete right: move the playhead over a clip"}
          disabled={locked || !canSplit}
          onClick={() => deleteSide("right")}
        >
          <PanelRightClose size={16} />
        </IconButton>
        <IconButton label={locked ? lockedLabel("Delete") : deleteLabel} disabled={locked || (selection.length === 0 && !cut)} onClick={deleteSelection}>
          <Trash2 size={16} />
        </IconButton>
        <IconButton
          label={locked ? lockedLabel("Duplicate") : selection.length ? "Duplicate (Ctrl+D)" : "Duplicate: select a clip first"}
          disabled={locked || selection.length === 0}
          onClick={duplicateSelection}
        >
          <Copy size={15} />
        </IconButton>
        <span className="mx-1 h-5 w-px bg-line" />
        <IconButton label={snapping ? "Snapping on" : "Snapping off"} active={snapping} onClick={() => setSnapping(!snapping)}>
          <Magnet size={16} />
        </IconButton>
        <div className="flex-1" />
        <IconButton label="Zoom out (-)" onClick={() => setZoom(zoom / 1.3)}>
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
          className="w-28"
        />
        <IconButton label="Zoom in (+)" onClick={() => setZoom(zoom * 1.3)}>
          <ZoomIn size={16} />
        </IconButton>
        <IconButton
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

      <div ref={scroller} className="relative min-h-0 flex-1 overflow-auto" onScroll={(e) => setView({ left: e.currentTarget.scrollLeft, width: e.currentTarget.clientWidth })}>
        <div className="relative" style={{ width: HEADER_W + laneWidth, minHeight: "100%" }}>
          {/* Ruler */}
          <div className="sticky top-0 z-40 flex" style={{ height: RULER_H }}>
            <div className="sticky left-0 z-50 shrink-0 border-b border-r border-line bg-panel" style={{ width: HEADER_W }} />
            <div className="relative flex-1 cursor-pointer border-b border-line bg-panel" onPointerDown={startScrub}>
              {ticks.map((t) => (
                <div key={t} className="absolute top-0 h-full" style={{ left: t * zoom }}>
                  <div className="h-2.5 w-px bg-subtle" />
                  <span className="tabular absolute left-1 top-2.5 text-[11px] text-muted">{formatTime(t * US, step < 1)}</span>
                  {[1, 2, 3, 4].map((i) => (
                    <div key={i} className="absolute top-0 h-1.5 w-px bg-line" style={{ left: (i * step * zoom) / 5 }} />
                  ))}
                </div>
              ))}
              {pastEnd && (
                <div className="absolute inset-y-0 w-2 -translate-x-1/2" style={{ left: endX }} title={END_TIP}>
                  <div className="mx-auto h-full w-0 border-l border-dashed border-muted" />
                </div>
              )}
            </div>
          </div>

          {/* Tracks */}
          <div className="relative flex flex-col gap-1 py-1">
            {drag?.moved && drag.target === null && dragKind !== "audio" && <div className="ml-[132px] h-0.5 bg-accent" title="Drop to create a new track" />}
            {tracks.map((track) => {
              const h = rowHeight(track);
              const isMain = track.id === MAIN_TRACK;
              return (
                <div key={track.id} className="flex" style={{ height: h }}>
                  <TrackHeader track={track} width={HEADER_W} locked={locked} />
                  <div
                    ref={(el) => {
                      if (el) rows.current.set(track.id, el);
                      else rows.current.delete(track.id);
                    }}
                    className={`relative flex-1 ${isMain ? "bg-white/[0.035]" : "bg-white/[0.015]"} ${assetDrag && dropTime !== null ? "outline-1 outline-dashed outline-line" : ""}`}
                    onPointerDown={(e) => {
                      if (e.target === e.currentTarget) {
                        select([]);
                        startScrub(e);
                      }
                    }}
                  >
                    {isMain && emptyTimeline && (
                      <div className="pointer-events-none absolute inset-1 flex items-center justify-center rounded-md border border-dashed border-line text-[12px] text-muted">
                        Drag media here, or press + on a media item
                      </div>
                    )}
                    {track.clips.map((clip) =>
                      drag?.moved && drag.clip.id === clip.id ? (
                        <div
                          key={clip.id}
                          className="absolute top-1 bottom-1 rounded-md border border-dashed border-muted/50"
                          style={{ left: (clip.startUs / US) * zoom + 1, width: (clip.durationUs / US) * zoom - 2 }}
                        />
                      ) : (
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
                          onPointerDown={startClipDrag}
                          onContextMenu={openMenu}
                        />
                      ),
                    )}
                    {isMain && !drag?.moved && <CutMarkers project={project} zoom={zoom} locked={locked} />}
                    {drag?.moved && ghostTiming && ghostTrackId === track.id && (
                      <ClipView clip={drag.clip} track={track} project={project} zoom={zoom} height={h} visible={visible} selected ghost timing={ghostTiming} />
                    )}
                  </div>
                </div>
              );
            })}
            {drag?.moved && drag.target === null && dragKind === "audio" && <div className="ml-[132px] h-0.5 bg-accent" />}
          </div>

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
            <div className="tabular pointer-events-none absolute z-50 rounded bg-black/85 px-1.5 py-0.5 text-[11px] text-fg" style={{ left: HEADER_W + (ghostTiming.startUs / US) * zoom, top: 2 }}>
              {drag.mode === "move" ? formatTime(ghostTiming.startUs) : formatDuration(ghostTiming.durationUs)}
            </div>
          )}
        </div>
      </div>
      {menu && <ClipMenu at={menu} onClose={closeMenu} />}
    </section>
  );
}
