import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  AudioLines,
  Captions,
  Eye,
  EyeOff,
  Film,
  Magnet,
  Maximize2,
  Scissors,
  Trash2,
  Type,
  Volume2,
  VolumeX,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { MAIN_TRACK, allClips, displayTracks, projectDuration, useEditor } from "../lib/store";
import { US, formatDuration, formatTime } from "../lib/time";
import type { Asset, Clip, Project, Track } from "../lib/types";
import { setDropResolver } from "./MediaPanel";
import { IconButton } from "./ui";

const HEADER_W = 132;
const RULER_H = 28;
const EDGE = 7;
const SNAP_PX = 8;

function rowHeight(track: Track) {
  if (track.id === MAIN_TRACK) return 64;
  if (track.kind === "video") return 50;
  if (track.kind === "audio") return 46;
  return 34;
}

type Mode = "move" | "trimL" | "trimR";

interface Drag {
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
}

function clipAsset(project: Project, clip: Clip): Asset | undefined {
  const c = clip.content;
  return c.type === "media" ? project.assets.find((a) => a.id === c.assetId) : undefined;
}

function clipKind(project: Project, clip: Clip): Track["kind"] {
  if (clip.content.type === "text") return "text";
  return clipAsset(project, clip)?.kind === "audio" ? "audio" : "video";
}

/** Clip timing while a drag is in progress. */
function dragResult(d: Drag, minUs: number): { startUs: number; durationUs: number } {
  const { clip } = d;
  if (d.mode === "move") return { startUs: Math.max(0, clip.startUs + d.dxUs), durationUs: clip.durationUs };
  if (d.mode === "trimL") {
    const lower = d.sourceInUs !== null ? -d.sourceInUs : -clip.startUs;
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

function tickStep(zoom: number): number {
  for (const s of [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600]) if (s * zoom >= 72) return s;
  return 1200;
}

function Waveform({ assetId, sourceInUs, durationUs, width, color }: { assetId: string; sourceInUs: number; durationUs: number; width: number; color: string }) {
  const peaks = useEditor((s) => s.waveforms[assetId]);
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => useEditor.getState().loadWaveform(assetId), [assetId]);
  useEffect(() => {
    const el = ref.current;
    if (!el || !peaks) return;
    const w = Math.max(1, Math.min(4096, Math.round(width)));
    const h = el.clientHeight || 24;
    el.width = w;
    el.height = h;
    const ctx = el.getContext("2d")!;
    ctx.clearRect(0, 0, w, h);
    ctx.fillStyle = color;
    const per = 50; // peaks per second
    const first = (sourceInUs / US) * per;
    const span = (durationUs / US) * per;
    for (let x = 0; x < w; x++) {
      const a = Math.floor(first + (x / w) * span);
      const b = Math.max(a + 1, Math.floor(first + ((x + 1) / w) * span));
      let m = 0;
      for (let i = a; i < b && i < peaks.length; i++) m = Math.max(m, peaks[i]);
      // Square root keeps quiet audio visible, roughly like a dB scale.
      const bar = Math.max(1, Math.sqrt(m / 255) * h);
      ctx.fillRect(x, h - bar, 1, bar);
    }
  }, [peaks, sourceInUs, durationUs, width, color]);
  if (!peaks) return null;
  return <canvas ref={ref} className="pointer-events-none absolute inset-x-0 bottom-0 h-[60%] w-full opacity-80" />;
}

function ClipView({
  clip,
  track,
  project,
  zoom,
  selected,
  ghost,
  timing,
  onPointerDown,
}: {
  clip: Clip;
  track: Track;
  project: Project;
  zoom: number;
  selected: boolean;
  ghost?: boolean;
  timing?: { startUs: number; durationUs: number };
  onPointerDown?: (e: React.PointerEvent, clip: Clip, track: Track) => void;
}) {
  const asset = clipAsset(project, clip);
  const thumb = useEditor((s) => (asset ? s.thumbs[asset.id] : undefined));
  useEffect(() => {
    if (asset && asset.kind !== "audio") useEditor.getState().loadThumb(asset.id);
  }, [asset]);
  const { startUs, durationUs } = timing ?? clip;
  const left = (startUs / US) * zoom;
  const width = Math.max(2, (durationUs / US) * zoom);
  const isCaption = clip.content.type === "text" && track.name === "Captions";
  const bg =
    clip.content.type === "text"
      ? isCaption
        ? "bg-clip-captions"
        : "bg-clip-text"
      : asset?.kind === "audio"
        ? "bg-clip-audio"
        : asset?.kind === "image"
          ? "bg-clip-image"
          : "bg-clip-video";
  const Icon = clip.content.type === "text" ? (isCaption ? Captions : Type) : asset?.kind === "audio" ? AudioLines : Film;
  const label = clip.content.type === "text" ? clip.content.text : (asset?.name ?? "Missing media");
  const showThumb = thumb && asset && asset.kind !== "audio";

  return (
    <div
      data-clip-id={clip.id}
      onPointerDown={onPointerDown ? (e) => onPointerDown(e, clip, track) : undefined}
      className={`group absolute top-1 bottom-1 overflow-hidden rounded-md ${bg} ${
        ghost ? "z-30 opacity-85 shadow-xl shadow-black/60 ring-2 ring-accent" : selected ? "z-10 ring-2 ring-accent" : "ring-1 ring-black/40 hover:ring-muted/60"
      } ${track.hidden ? "opacity-40" : ""} cursor-grab active:cursor-grabbing`}
      style={{
        left,
        width,
        backgroundImage: showThumb ? `url(${thumb})` : undefined,
        backgroundSize: showThumb ? "auto 100%" : undefined,
        backgroundRepeat: "repeat-x",
      }}
      aria-label={`${label}, ${formatDuration(durationUs)}`}
    >
      {asset?.kind === "audio" && clip.content.type === "media" && (
        <Waveform assetId={asset.id} sourceInUs={clip.content.sourceInUs} durationUs={durationUs} width={width} color="#5fd3a5" />
      )}
      <div className={`pointer-events-none flex items-center gap-1 px-1.5 py-0.5 text-[11px] text-fg ${showThumb ? "bg-gradient-to-b from-black/70 to-transparent" : ""}`}>
        <Icon size={11} className="shrink-0" />
        <span className="truncate">{label}</span>
        {width > 110 && <span className="tabular ml-auto shrink-0 pl-1 text-[10px] text-fg/70">{formatDuration(durationUs)}</span>}
      </div>
      {/* Trim handles: visible on hover and selection so edges look grabbable. */}
      <div className={`absolute inset-y-0 left-0 w-[7px] cursor-ew-resize rounded-l-md bg-white/0 ${selected ? "bg-white/90" : "group-hover:bg-white/50"}`} />
      <div className={`absolute inset-y-0 right-0 w-[7px] cursor-ew-resize rounded-r-md bg-white/0 ${selected ? "bg-white/90" : "group-hover:bg-white/50"}`} />
    </div>
  );
}

export function Timeline() {
  const snap0 = useEditor((s) => s.snap);
  const project = snap0?.project;
  const selection = useEditor((s) => s.selection);
  const timeUs = useEditor((s) => s.timeUs);
  const playing = useEditor((s) => s.playing);
  const zoom = useEditor((s) => s.zoom);
  const assetDrag = useEditor((s) => s.assetDrag);
  const { select, seek, edit, setZoom } = useEditor.getState();
  const scroller = useRef<HTMLDivElement>(null);
  const rows = useRef(new Map<string, HTMLDivElement>());
  const [drag, setDrag] = useState<Drag | null>(null);
  const [snapping, setSnapping] = useState(true);
  const [view, setView] = useState({ left: 0, width: 1000 });

  const tracks = useMemo(() => (project ? displayTracks(project) : []), [project]);
  const duration = project ? projectDuration(project) : 0;
  const fps = project?.canvas.fps ?? 30;
  const minUs = Math.ceil(US / fps);
  const laneWidth = Math.max(view.width - HEADER_W, ((duration / US) + 30) * zoom);

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

  // Keep the playhead visible while playing.
  useEffect(() => {
    const el = scroller.current;
    if (!el || !playing) return;
    const x = (timeUs / US) * zoom;
    const visible = el.clientWidth - HEADER_W;
    if (x > el.scrollLeft + visible - 24 || x < el.scrollLeft) el.scrollLeft = Math.max(0, x - 48);
  }, [timeUs, playing, zoom]);

  // Ctrl + wheel zooms around the pointer.
  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      const z = useEditor.getState().zoom;
      const rect = el.getBoundingClientRect();
      const px = e.clientX - rect.left - HEADER_W;
      const t = (el.scrollLeft + px) / z;
      const next = Math.max(4, Math.min(600, z * (e.deltaY < 0 ? 1.15 : 1 / 1.15)));
      setZoom(next);
      requestAnimationFrame(() => (el.scrollLeft = Math.max(0, t * next - px)));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [setZoom]);

  const startClipDrag = (e: React.PointerEvent, clip: Clip, track: Track) => {
    if (e.button !== 0 || !project) return;
    e.stopPropagation();
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const x = e.clientX - rect.left;
    const mode: Mode = x <= EDGE ? "trimL" : x >= rect.width - EDGE ? "trimR" : "move";
    const asset = clipAsset(project, clip);
    const sourceInUs = clip.content.type === "media" && asset?.kind !== "image" ? clip.content.sourceInUs : null;
    const maxDurUs = sourceInUs !== null && asset ? asset.durationUs - sourceInUs : null;
    const candidates = [0, useEditor.getState().timeUs];
    for (const c of allClips(project)) if (c.id !== clip.id) candidates.push(c.startUs, c.startUs + c.durationUs);
    setDrag({ clip, trackId: track.id, mode, startX: e.clientX, startY: e.clientY, moved: false, shift: e.shiftKey, dxUs: 0, target: undefined, snapUs: null, candidates, maxDurUs, sourceInUs });
  };

  useEffect(() => {
    if (!drag || !project) return;
    const kind = clipKind(project, drag.clip);
    const onMove = (e: PointerEvent) => {
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
        const sourceInUs = d.mode === "trimL" && d.sourceInUs !== null ? d.sourceInUs + (r.startUs - d.clip.startUs) : null;
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
  }, [drag, project, zoom, snapping, minUs, edit, select]);

  // Scrubbing on the ruler and on empty lane space.
  const startScrub = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    seek(timeAt(e.clientX));
    let raf = 0;
    let lastX = e.clientX;
    const move = (ev: PointerEvent) => {
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

  if (!project) return <section className="h-[300px] border-t border-line bg-panel" />;

  const splitTargets = (() => {
    const under = (c: Clip) => timeUs > c.startUs + minUs && timeUs < c.startUs + c.durationUs - minUs;
    const sel = allClips(project).filter((c) => selection.includes(c.id) && under(c));
    if (sel.length > 0) return sel;
    return (project.tracks.find((t) => t.id === MAIN_TRACK)?.clips ?? []).filter(under);
  })();

  const step = tickStep(zoom);
  const firstTick = Math.floor(Math.max(0, view.left - 100) / zoom / step) * step;
  const lastTick = (view.left + view.width + 100) / zoom;
  const ticks: number[] = [];
  for (let t = firstTick; t <= lastTick; t += step) ticks.push(Math.round(t * 1000) / 1000);

  const ghostTiming = drag?.moved ? dragResult(drag, minUs) : null;
  const ghostTrackId = drag?.moved ? (drag.target === undefined ? drag.trackId : drag.target) : null;
  const dropTime = assetDrag && scroller.current ? (() => {
    const r = scroller.current.getBoundingClientRect();
    return assetDrag.x >= r.left && assetDrag.x <= r.right && assetDrag.y >= r.top && assetDrag.y <= r.bottom ? timeAt(assetDrag.x) : null;
  })() : null;
  const emptyTimeline = allClips(project).length === 0;

  return (
    <section className="flex h-[300px] shrink-0 flex-col border-t border-line bg-panel" aria-label="Timeline">
      <div className="flex h-10 shrink-0 items-center gap-1 border-b border-line px-2">
        <IconButton
          label={splitTargets.length ? "Split at playhead (S)" : "Split: move the playhead over a clip"}
          disabled={splitTargets.length === 0}
          onClick={async () => {
            for (const c of splitTargets) await edit({ type: "splitClip", clipId: c.id, atUs: Math.round(timeUs) });
          }}
        >
          <Scissors size={16} />
        </IconButton>
        <IconButton
          label={selection.length ? "Delete selection (Delete)" : "Delete: select a clip first"}
          disabled={selection.length === 0}
          onClick={() => edit({ type: "deleteClips", clipIds: selection }).then((s) => s && select([]))}
        >
          <Trash2 size={16} />
        </IconButton>
        <span className="mx-1 h-5 w-px bg-line" />
        <IconButton label={snapping ? "Snapping on" : "Snapping off"} active={snapping} onClick={() => setSnapping(!snapping)}>
          <Magnet size={16} />
        </IconButton>
        <div className="flex-1" />
        <span className="tabular mr-2 text-[12px] text-muted">
          {formatTime(timeUs)} <span className="text-subtle">/ {formatTime(duration)}</span>
        </span>
        <IconButton label="Zoom out (-)" onClick={() => setZoom(zoom / 1.3)}>
          <ZoomOut size={16} />
        </IconButton>
        <input
          type="range"
          aria-label="Timeline zoom"
          min={Math.log(4)}
          max={Math.log(600)}
          step={0.01}
          value={Math.log(zoom)}
          onChange={(e) => setZoom(Math.exp(Number(e.target.value)))}
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
            setZoom(w / Math.max(1, duration / US));
            if (scroller.current) scroller.current.scrollLeft = 0;
          }}
        >
          <Maximize2 size={15} />
        </IconButton>
      </div>

      <div
        ref={scroller}
        className="relative min-h-0 flex-1 overflow-auto"
        onScroll={(e) => setView({ left: e.currentTarget.scrollLeft, width: e.currentTarget.clientWidth })}
      >
        <div className="relative" style={{ width: HEADER_W + laneWidth, minHeight: "100%" }}>
          {/* Ruler */}
          <div className="sticky top-0 z-40 flex" style={{ height: RULER_H }}>
            <div className="sticky left-0 z-50 shrink-0 border-b border-r border-line bg-panel" style={{ width: HEADER_W }} />
            <div className="relative flex-1 cursor-pointer border-b border-line bg-panel" onPointerDown={startScrub}>
              {ticks.map((t) => (
                <div key={t} className="absolute top-0 h-full" style={{ left: t * zoom }}>
                  <div className="h-2.5 w-px bg-subtle" />
                  <span className="tabular absolute left-1 top-2.5 text-[10px] text-subtle">{formatTime(t * US, step < 1)}</span>
                  {[1, 2, 3, 4].map((i) => (
                    <div key={i} className="absolute top-0 h-1.5 w-px bg-line" style={{ left: (i * step * zoom) / 5 }} />
                  ))}
                </div>
              ))}
            </div>
          </div>

          {/* Tracks */}
          <div className="relative flex flex-col gap-1 py-1">
            {drag?.moved && drag.target === null && clipKind(project, drag.clip) !== "audio" && (
              <div className="ml-[132px] h-0.5 bg-accent" title="Drop to create a new track" />
            )}
            {tracks.map((track) => {
              const h = rowHeight(track);
              const isMain = track.id === MAIN_TRACK;
              const KindIcon = track.kind === "audio" ? AudioLines : track.kind === "text" ? (track.name === "Captions" ? Captions : Type) : Film;
              return (
                <div key={track.id} className="flex" style={{ height: h }}>
                  <div
                    className="sticky left-0 z-20 flex shrink-0 items-center gap-1 border-r border-line bg-panel pl-2 pr-1"
                    style={{ width: HEADER_W }}
                  >
                    <KindIcon size={13} className="shrink-0 text-muted" />
                    <span className={`flex-1 truncate text-[12px] ${isMain ? "font-medium text-fg" : "text-muted"}`}>{track.name || track.kind}</span>
                    {track.kind !== "audio" && (
                      <IconButton
                        label={track.hidden ? `Show ${track.name}` : `Hide ${track.name}`}
                        className="h-7 w-7"
                        active={track.hidden}
                        onClick={() => edit({ type: "updateTrack", trackId: track.id, hidden: !track.hidden })}
                      >
                        {track.hidden ? <EyeOff size={14} /> : <Eye size={14} />}
                      </IconButton>
                    )}
                    {track.kind !== "text" && (
                      <IconButton
                        label={track.muted ? `Unmute ${track.name}` : `Mute ${track.name}`}
                        className="h-7 w-7"
                        active={track.muted}
                        onClick={() => edit({ type: "updateTrack", trackId: track.id, muted: !track.muted })}
                      >
                        {track.muted ? <VolumeX size={14} /> : <Volume2 size={14} />}
                      </IconButton>
                    )}
                  </div>
                  <div
                    ref={(el) => {
                      if (el) rows.current.set(track.id, el);
                      else rows.current.delete(track.id);
                    }}
                    className={`relative flex-1 ${isMain ? "bg-white/[0.035]" : "bg-white/[0.015]"} ${
                      assetDrag && dropTime !== null ? "outline-1 outline-dashed outline-line" : ""
                    }`}
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
                          style={{ left: (clip.startUs / US) * zoom, width: (clip.durationUs / US) * zoom }}
                        />
                      ) : (
                        <ClipView
                          key={clip.id}
                          clip={clip}
                          track={track}
                          project={project}
                          zoom={zoom}
                          selected={selection.includes(clip.id)}
                          onPointerDown={startClipDrag}
                        />
                      ),
                    )}
                    {drag?.moved && ghostTiming && ghostTrackId === track.id && (
                      <ClipView clip={drag.clip} track={track} project={project} zoom={zoom} selected ghost timing={ghostTiming} />
                    )}
                  </div>
                </div>
              );
            })}
            {drag?.moved && drag.target === null && clipKind(project, drag.clip) === "audio" && <div className="ml-[132px] h-0.5 bg-accent" />}
          </div>

          {/* Ghost for a drag onto a new track follows the pointer row-less. */}
          {drag?.moved && drag.target === null && ghostTiming && (
            <div className="pointer-events-none absolute z-30" style={{ left: HEADER_W, top: RULER_H, height: 44, width: laneWidth }}>
              <ClipView clip={drag.clip} track={project.tracks.find((t) => t.id === drag.trackId)!} project={project} zoom={zoom} selected ghost timing={ghostTiming} />
            </div>
          )}

          {/* Snap guide */}
          {drag?.moved && drag.snapUs !== null && (
            <div className="pointer-events-none absolute bottom-0 top-0 z-30 w-px bg-warn" style={{ left: HEADER_W + (drag.snapUs / US) * zoom }} />
          )}

          {/* Drop position for media dragged from the panel */}
          {dropTime !== null && (
            <div className="pointer-events-none absolute bottom-0 top-0 z-30 w-0.5 bg-accent" style={{ left: HEADER_W + (dropTime / US) * zoom }} />
          )}

          {/* Playhead */}
          <div className="pointer-events-none absolute bottom-0 top-0 z-[45]" style={{ left: HEADER_W + (timeUs / US) * zoom }}>
            <div className="absolute -left-[5px] top-0 h-3 w-[11px] rounded-b-sm bg-fg" />
            <div className="absolute left-0 top-0 h-full w-px bg-fg" />
          </div>

          {drag?.moved && ghostTiming && (
            <div
              className="tabular pointer-events-none absolute z-50 rounded bg-black/85 px-1.5 py-0.5 text-[11px] text-fg"
              style={{ left: HEADER_W + (ghostTiming.startUs / US) * zoom, top: 2 }}
            >
              {drag.mode === "move" ? formatTime(ghostTiming.startUs) : formatDuration(ghostTiming.durationUs)}
            </div>
          )}
        </div>
      </div>
    </section>
  );
}
