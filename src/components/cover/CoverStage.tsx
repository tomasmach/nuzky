import { useEffect, useRef, useState } from "react";
import { AlertCircle, AlertTriangle, Loader2, MonitorPlay, Smartphone } from "lucide-react";
import { api, errorText, plainError } from "../../lib/api";
import { COVER_FORMATS, chooseFrame, closeCover, coverFormat, downloadModels, editCover, editText, mask, newCover, setSafeZones, switchFormat, thumbnailOf, useCover } from "../../lib/cover";
import { projectDuration, useEditor } from "../../lib/store";
import { formatTime } from "../../lib/time";
import type { CoverView, LayerBounds, Snapshot, Thumbnail, ThumbnailFormat, Transform } from "../../lib/types";
import { LayerBox, type LayerSource } from "../preview/LayerOverlay";
import { Button, IconButton, Segmented } from "../ui";

/** A text the person covers more of than this gets a warning: the hook should still read at a glance. */
export const COVERED_TOO_MUCH = 0.3;

/** The cover being edited: the project's, or the one the editor offers at the playhead before there is any. */
export function useShownCover(format: ThumbnailFormat): { cover: Thumbnail; draft: boolean } | null {
  const project = useEditor((s) => s.snap?.project);
  const timeUs = useEditor((s) => s.timeUs);
  const models = useCover((s) => s.models);
  if (!project || projectDuration(project) <= 0) return null;
  const cover = thumbnailOf(project, format);
  if (cover) return { cover, draft: false };
  // Recomputed per render: `models` decides whether a YouTube thumbnail of a vertical video starts blurred.
  void models;
  return { cover: newCover(format, project, Math.min(timeUs, Math.max(0, projectDuration(project) - 1))), draft: true };
}

/** What Reels, the profile grid and YouTube cover of a thumbnail, in its own pixels. */
function SafeZones({ format }: { format: ThumbnailFormat }) {
  const { width, height } = coverFormat(format);
  const pct = (v: number, of: number) => `${(v / of) * 100}%`;
  const band = "pointer-events-none absolute bg-black/45";
  const chip = "pointer-events-none absolute rounded bg-black/60 px-1.5 text-[11px] leading-[18px] text-fg";
  if (format === "youtube_16x9")
    return (
      <div className={`${band} bottom-0 right-0 flex items-center justify-center border-l border-t border-dashed border-fg/50`} style={{ width: "20%", height: "20%" }}>
        <span className="rounded bg-black/60 px-1.5 text-[11px] leading-[18px] text-fg">Duration</span>
      </div>
    );
  // Reels' safe area of a 1080 × 1920 video, as the preview's safe zone shows it, and the 3:4 middle the profile grid shows.
  const area = { left: 60, top: 250, right: 900, bottom: 1420 };
  const grid = (height - (width * 4) / 3) / 2;
  return (
    <>
      <div className={`${band} inset-x-0 top-0`} style={{ height: pct(area.top, height) }} />
      <div className={`${band} inset-x-0 bottom-0`} style={{ height: pct(height - area.bottom, height) }} />
      <div className={`${band} left-0`} style={{ top: pct(area.top, height), bottom: pct(height - area.bottom, height), width: pct(area.left, width) }} />
      <div className={`${band} right-0`} style={{ top: pct(area.top, height), bottom: pct(height - area.bottom, height), width: pct(width - area.right, width) }} />
      <div
        className="pointer-events-none absolute border border-dashed border-fg/50"
        style={{ left: pct(area.left, width), top: pct(area.top, height), width: pct(area.right - area.left, width), height: pct(area.bottom - area.top, height) }}
      />
      <div className="pointer-events-none absolute inset-x-0 border-t border-dashed border-fg/70" style={{ top: pct(grid, height) }} />
      <div className="pointer-events-none absolute inset-x-0 border-t border-dashed border-fg/70" style={{ top: pct(height - grid, height) }} />
      {/* Under the grid's lower line, in the band Reels covers anyway, clear of where the hook goes. */}
      <span className={`${chip} left-1`} style={{ top: `calc(${pct(height - grid, height)} + 4px)` }}>
        Profile grid
      </span>
    </>
  );
}

/** Asks the engine for the cover one request at a time; a change while one renders asks again once it is done. */
function useCoverView(format: ThumbnailFormat, width: number, draft: Thumbnail | null) {
  const revision = useEditor((s) => s.snap?.revision ?? 0);
  const masks = useCover((s) => s.masks);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [view, setView] = useState<CoverView | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** The project revision the cover on screen was drawn from. */
  const [drawnFrom, setDrawnFrom] = useState(-1);
  const state = useRef({ busy: false, again: false, masks: -1, shown: -1, waiters: [] as { revision: number; done: () => void }[] });
  const args = useRef({ format, width, draft, revision, masks });
  args.current = { format, width, draft, revision, masks };
  const draftKey = draft ? JSON.stringify(draft) : "";

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    const s = state.current;
    // One request at a time; the newest arguments are read when it starts, and a change meanwhile asks again.
    const request = () => {
      if (s.busy) return void (s.again = true);
      s.busy = true;
      const a = args.current;
      const refresh = a.masks !== s.masks;
      api
        .coverView(a.format, a.width, refresh, a.draft)
        .then(({ view, rgba }) => {
          if (!mounted.current) return;
          s.masks = a.masks;
          const el = canvas.current;
          if (el) {
            if (el.width !== view.width || el.height !== view.height) {
              el.width = view.width;
              el.height = view.height;
            }
            el.getContext("2d")?.putImageData(new ImageData(rgba, view.width, view.height), 0, 0);
          }
          setView(view);
          setDrawnFrom(a.revision);
          setError(null);
          useCover.setState({ hidden: view.maskMissing ? [] : view.hidden });
        })
        .catch((e) => mounted.current && setError(plainError(errorText(e))))
        .finally(() => {
          s.busy = false;
          s.shown = a.revision;
          s.waiters = s.waiters.filter((w) => (w.revision <= a.revision ? (w.done(), false) : true));
          if (mounted.current && s.again) {
            s.again = false;
            request();
          }
        });
    };
    request();
  }, [format, width, revision, masks, draftKey]);

  /** Resolves once a cover drawn from `revision` or later is on screen. */
  const shownFrom = (revision: number) => (state.current.shown >= revision ? Promise.resolve() : new Promise<void>((done) => state.current.waiters.push({ revision, done })));
  return { canvas, view, error, shownFrom, drawnFrom };
}

/** The cover in place of the video: drawn by the engine as the export draws it, with its texts and frame to drag. */
export function CoverStage({ format, box }: { format: ThumbnailFormat; box: { w: number; h: number } }) {
  const { width: tw, height: th } = coverFormat(format);
  const aspect = tw / th;
  const fit = box.w / box.h > aspect ? { h: box.h, w: box.h * aspect } : { w: box.w, h: box.w / aspect };
  const shown = useShownCover(format);
  const dpr = window.devicePixelRatio || 1;
  // In steps, so resizing the window does not ask for a new cover on every pixel.
  const width = Math.max(16, Math.min(tw, Math.ceil((fit.w * dpr) / 32) * 32));
  const { canvas, view, error, shownFrom, drawnFrom } = useCoverView(format, width, shown?.draft ? shown.cover : null);
  const selected = useCover((s) => s.selected);
  const safeZones = useCover((s) => s.safeZones);
  const k = fit.w / tw;

  const layers: LayerBounds[] = view
    ? [
        ...(shown && !shown.draft ? [{ clipId: "frame", corners: view.frame[0], frame: view.frame[1] }] : []),
        ...view.bounds.flatMap((b, i) => (b ? [{ clipId: String(i), corners: b, frame: b }] : [])),
      ]
    : [];
  const latest = useRef(layers);
  latest.current = layers;
  const cover = () => thumbnailOf(useEditor.getState().snap?.project, format);
  const source: LayerSource = {
    canvas: { width: tw, height: th },
    bounds: layers,
    current: () => Promise.resolve(latest.current),
    settled: (last) =>
      (last as Promise<Snapshot | null>)
        .then((snap) => (snap ? shownFrom(snap.revision) : undefined))
        .then(() => latest.current),
    onBounds: () => {},
    selected: selected === null ? null : String(selected),
    isOnlySelected: (id) => String(useCover.getState().selected) === id,
    select: (id) => useCover.setState({ selected: id === null ? null : id === "frame" ? "frame" : Number(id) }),
    transform: (id) => {
      const c = cover();
      return (id === "frame" ? c?.frame : c?.texts[Number(id)]?.transform) ?? null;
    },
    change: (id, patch: Partial<Transform>, key) =>
      id === "frame" ? editCover(format, (c) => (c ? { ...c, frame: { ...c.frame, ...patch } } : null), key) : editText(format, Number(id), (t) => ({ ...t, transform: { ...t.transform, ...patch } }), key),
    croppable: () => false,
    radius: () => 0,
    locked: () => !!useEditor.getState().aiRun,
    hidden: false,
  };

  if (!shown)
    return (
      <div className="absolute inset-0 flex items-center justify-center p-6 text-center text-[13px] text-muted">Add a video to the timeline to make a cover.</div>
    );
  const covered = view && !view.maskMissing ? view.bounds.map((b, i) => (b && view.hidden[i] > COVERED_TOO_MUCH ? b : null)) : [];
  return (
    <div className="absolute inset-0 flex items-center justify-center">
      <div
        className="relative"
        style={{ width: fit.w, height: fit.h }}
        data-testid="cover-stage"
        data-time={view?.timeUs}
        data-revision={drawnFrom}
        data-mask={view?.maskMissing ? "missing" : "ready"}
      >
        {/* Square corners and only a hairline, as the video's frame. */}
        <div className="absolute inset-0 overflow-hidden bg-black shadow-[0_0_0_1px_rgb(255_255_255/.07)]">
          <canvas ref={canvas} className="h-full w-full" aria-label={`${coverFormat(format).label} as it exports`} data-testid="cover-canvas" />
          {safeZones && <SafeZones format={format} />}
          {!view && !error && <div className="skeleton absolute inset-0 opacity-60" aria-label="Drawing the cover" />}
          {error && (
            <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-black/80 p-6 text-center" role="alert">
              <AlertCircle size={18} className="text-danger" />
              <p className="text-[13px] text-fg">{error}</p>
            </div>
          )}
        </div>
        {covered.map(
          (b, i) =>
            b && (
              <span
                key={i}
                // Just outside the text's top right corner, clear of the selection box's handle there, and inside the cover.
                className="pointer-events-none absolute flex h-4 w-4 items-center justify-center"
                style={{
                  left: Math.min(fit.w - 20, Math.max(...b.map((p) => p[0])) * k + 6),
                  top: Math.max(4, Math.min(...b.map((p) => p[1])) * k - 22),
                }}
                title="The person covers too much of this text"
              >
                <AlertTriangle size={16} className="text-warn drop-shadow-[0_0_1px_rgb(0_0_0)]" fill="rgb(0 0 0 / .55)" />
              </span>
            ),
        )}
        {view && <LayerBox width={fit.w} height={fit.h} bleed={{ x: (box.w - fit.w) / 2, y: (box.h - fit.h) / 2 }} source={source} />}
        <StatusChip format={format} view={view} draft={shown.draft} />
      </div>
    </div>
  );
}

/** One chip at the top left of the cover for what it is waiting for, with one way on. */
function StatusChip({ format, view, draft }: { format: ThumbnailFormat; view: CoverView | null; draft: boolean }) {
  const models = useCover((s) => s.models);
  const job = useEditor((s) => {
    const { maskJob, modelsJob } = useCover.getState();
    const id = [maskJob, modelsJob].find((j) => j && s.jobs[j]?.status === "running");
    return id ? s.jobs[id] : null;
  });
  const failed = useCover((s) => s.failed);
  const damaged = useCover((s) => s.damaged);
  const locked = useEditor((s) => !!s.aiRun);
  const timeUs = useEditor((s) => s.timeUs);
  const missing = !!view?.maskMissing;
  const tried = useRef(new Set<number>());

  // The person is cut out a moment after the frame stops changing, when the models are there.
  useEffect(() => {
    if (!view || !missing || !models?.downloaded || job || locked) return;
    const t = view.timeUs;
    if (tried.current.has(t)) return;
    const timer = window.setTimeout(() => {
      tried.current.add(t);
      void mask(t);
    }, 500);
    return () => window.clearTimeout(timer);
  }, [view, missing, models, job, locked]);

  const chip = "absolute left-2 top-2 z-10 flex h-7 max-w-[calc(100%-16px)] items-center gap-1.5 rounded-lg bg-black/70 pl-2 text-[12px] text-fg";
  if (draft)
    return (
      <div className={`${chip} pr-1`}>
        <span className="truncate">Not made yet</span>
        <Button pill className="h-5 shrink-0 px-2 text-[11px]" onClick={() => void chooseFrame(format, timeUs)}>
          Use this frame
        </Button>
      </div>
    );
  if (job)
    return (
      <div className={`${chip} pr-1`} role="status">
        <Loader2 size={13} className="shrink-0 animate-spin text-accent" />
        <span className="tabular truncate">
          {job.phase ?? job.label}
          {job.progress > 0 ? ` ${Math.round(job.progress * 100)}%` : "…"}
        </span>
        <Button pill className="h-5 shrink-0 px-2 text-[11px]" onClick={() => void api.cancelJob(job.id)}>
          Stop
        </Button>
      </div>
    );
  if (!missing) return null;
  if (models?.unavailable)
    return (
      <div className={`${chip} pr-2`} title={models.unavailable}>
        <span className="truncate text-muted">Cutting out the person doesn't work on this computer</span>
      </div>
    );
  if (failed)
    return (
      <div className={`${chip} pr-1`} role="alert">
        <AlertCircle size={13} className="shrink-0 text-danger" />
        <span className="truncate" title={failed}>
          {failed}
        </span>
        <Button pill className="h-5 shrink-0 px-2 text-[11px]" onClick={() => (models?.downloaded && !damaged ? view && mask(view.timeUs) : downloadModels())}>
          {damaged ? "Download again" : "Try again"}
        </Button>
      </div>
    );
  if (models && !models.downloaded)
    return (
      <div className={`${chip} pr-1`}>
        <span className="truncate">Cutting out the person needs the cover models</span>
        <Button pill className="h-5 shrink-0 px-2 text-[11px]" onClick={() => void downloadModels()}>
          Download {models.sizeMb} MB
        </Button>
      </div>
    );
  return (
    <div className={`${chip} pr-1`}>
      <span className="truncate">Not cut out yet</span>
      <Button pill className="h-5 shrink-0 px-2 text-[11px]" onClick={() => view && void mask(view.timeUs)}>
        Cut out
      </Button>
    </div>
  );
}

/** In place of the transport: the frame's time, the format, the safe zones and Done. */
export function CoverBar({ format }: { format: ThumbnailFormat }) {
  const time = useEditor((s) => thumbnailOf(s.snap?.project, format)?.timeUs ?? s.timeUs);
  const safeZones = useCover((s) => s.safeZones);
  return (
    <div className="bar grid h-11 w-full max-w-[620px] grid-cols-[1fr_auto_1fr] items-center gap-2 rounded-full pl-[18px] pr-1.5" data-testid="cover-bar">
      <span className="tabular truncate text-[13px]">
        <span className="text-muted @max-[460px]:hidden">Frame </span>
        <span className="font-medium text-fg">{formatTime(time)}</span>
      </span>
      <div className="w-[200px] @max-[460px]:w-[112px]">
        <Segmented
          label="Cover format"
          value={format}
          onChange={switchFormat}
          options={COVER_FORMATS.map((f) => ({ id: f.id, label: <><span className="@max-[460px]:hidden">{f.label}</span><span className="hidden @max-[460px]:inline">{f.short}</span></>, title: `${f.width}×${f.height}` }))}
        />
      </div>
      <div className="flex items-center justify-end gap-1">
        <IconButton
          round
          label={`${safeZones ? "Hide" : "Show"} ${format === "cover_9x16" ? "what Reels and the profile grid cover" : "where YouTube shows the length"}`}
          active={safeZones}
          onClick={() => setSafeZones(!safeZones)}
        >
          {format === "cover_9x16" ? <Smartphone size={16} /> : <MonitorPlay size={16} />}
        </IconButton>
        <Button pill className="h-8 px-3.5" onClick={closeCover}>
          Done
        </Button>
      </div>
    </div>
  );
}
