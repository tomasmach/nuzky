import { useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { videoDir, join } from "@tauri-apps/api/path";
import { AlertCircle, AlertTriangle, AudioLines, CheckCircle2, ChevronDown, X } from "lucide-react";
import { api, errorText, plainError } from "../lib/api";
import { formatLabel } from "../lib/presets";
import { useCover } from "../lib/cover";
import { currentEpoch, projectDuration, useEditor } from "../lib/store";
import { CoverExport } from "./cover/CoverExport";
import { FormatShape } from "./home/Home";
import { US, formatTime } from "../lib/time";
import type { Canvas, Delivery, ExportRequest } from "../lib/types";
import { Button, IconButton, Menu, type MenuEntry, ProgressBar, Segmented, trapTab } from "./ui";

const RESOLUTIONS = [
  { id: 720, label: "720p" },
  { id: 1080, label: "1080p" },
  { id: 1440, label: "1440p" },
  { id: 2160, label: "4K" },
];
const FRAME_RATES = [24, 25, 30, 50, 60];
const QUALITIES: { id: ExportRequest["quality"]; label: string; bitsPerPixel: number }[] = [
  { id: "high", label: "High", bitsPerPixel: 0.12 },
  { id: "recommended", label: "Recommended", bitsPerPixel: 0.07 },
  { id: "small", label: "Smaller file", bitsPerPixel: 0.035 },
];
const AUDIO_BPS = 192_000;
const STORAGE_KEY = "nuzky.export";
/**
 * What each preset fixes, as `Delivery` in the engine's export.rs, which refuses anything else with it. `fps: null`
 * keeps the frame rate free, starting from the project's: YouTube wants the rate the video was shot at.
 */
const PRESETS: { id: Delivery; label: string; ratio: [number, number]; resolution: number; fps: number | null }[] = [
  { id: "reels", label: "Reels & TikTok", ratio: [9, 16], resolution: 1080, fps: 30 },
  { id: "shorts", label: "YouTube Shorts", ratio: [9, 16], resolution: 1080, fps: null },
  { id: "youtube_1080p", label: "YouTube 1080p", ratio: [16, 9], resolution: 1080, fps: null },
  { id: "youtube_4k", label: "YouTube 4K", ratio: [16, 9], resolution: 2160, fps: null },
  { id: "instagram_feed", label: "Instagram feed", ratio: [4, 5], resolution: 1080, fps: 30 },
  { id: "square", label: "Square", ratio: [1, 1], resolution: 1080, fps: 30 },
];
const presetOf = (id: Delivery) => PRESETS.find((p) => p.id === id)!;
const fits = (id: Delivery, canvas: Canvas) => {
  const [w, h] = presetOf(id).ratio;
  return canvas.width * h === canvas.height * w;
};
function blockedReason(id: Delivery | null | undefined, canvas: Canvas) {
  if (!id || fits(id, canvas)) return null;
  const { label, ratio } = presetOf(id);
  return `${label} needs a ${ratio.join(":")} video. Switch Ratio under the preview to ${ratio.join(":")}.`;
}
/** The settings a preset sets when picked: its own size and frame rate, the project's rate where it keeps one. */
function presetOptions(id: Delivery, canvas: Canvas): Partial<ExportRequest> {
  const { resolution, fps } = presetOf(id);
  return { resolution, fps: fps ?? (FRAME_RATES.includes(canvas.fps) ? canvas.fps : 30), quality: "recommended", preset: id };
}

/** Output size for a short side, keeping the canvas aspect and even dimensions. */
export function outputSize(canvas: Canvas, shortSide: number) {
  const short = Math.min(canvas.width, canvas.height);
  const long = Math.round((Math.max(canvas.width, canvas.height) * shortSide) / short / 2) * 2;
  return canvas.width <= canvas.height ? { w: shortSide, h: long } : { w: long, h: shortSide };
}

/** Typical H.264 bitrates at these CRFs; real size depends on how much the picture moves. */
function estimateBytes(w: number, h: number, fps: number, quality: ExportRequest["quality"], durationUs: number) {
  const bpp = QUALITIES.find((q) => q.id === quality)!.bitsPerPixel;
  return ((w * h * fps * bpp + AUDIO_BPS) * (durationUs / US)) / 8;
}

const fileName = (path: string) => path.split(/[\\/]/).pop() || path;

function formatBytes(b: number) {
  if (b >= 1e9) return `${(b / 1e9).toFixed(1)} GB`;
  return `${Math.max(1, Math.round(b / 1e6))} MB`;
}

/** Tab and Shift+Tab cycle through the dialog's controls, so focus never leaves it for the editor behind. */
function loadOptions(canvas: Canvas): ExportRequest {
  const short = Math.min(canvas.width, canvas.height);
  const fallback: ExportRequest = {
    resolution: RESOLUTIONS.some((r) => r.id === short) ? short : 1080,
    fps: FRAME_RATES.includes(canvas.fps) ? canvas.fps : 30,
    quality: "recommended",
    preset: null,
  };
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null") as Partial<ExportRequest> | null;
    const options = { ...fallback, ...(saved?.resolution && { resolution: saved.resolution }), ...(saved?.quality && { quality: saved.quality }) };
    // The preset comes back where it applies; a project in another format starts from its own settings.
    const preset = PRESETS.find((p) => p.id === saved?.preset)?.id;
    return preset && fits(preset, canvas) ? { ...options, ...presetOptions(preset, canvas), quality: options.quality } : options;
  } catch {
    return fallback;
  }
}

export function ExportDialog() {
  const open = useEditor((s) => s.exportOpen);
  const project = useEditor((s) => s.snap?.project);
  const jobId = useEditor((s) => s.exportJobId);
  const job = useEditor((s) => (s.exportJobId ? s.jobs[s.exportJobId] : undefined));
  const [options, setOptions] = useState<ExportRequest | null>(null);
  /** The video, or the cover: in the cover editor the dialog opens on the cover. */
  const [what, setWhat] = useState<"video" | "cover">("video");
  /** What the Cover side would write, for the line under the title. */
  const [coverFacts, setCoverFacts] = useState("");
  const [error, setError] = useState<string | null>(null);
  /** The file asked for already exists; Replace exports over it. */
  const [exists, setExists] = useState<string | null>(null);
  /** The credits file the video's CC BY sounds bring is already there; Replace writes over it. */
  const [creditsExist, setCreditsExist] = useState<string | null>(null);
  const last = useRef<{ path: string; replace: boolean; credits: boolean } | null>(null);
  const startedAt = useRef(0);
  const dialog = useRef<HTMLDivElement>(null);
  const [presetMenu, setPresetMenu] = useState<{ x: number; y: number; width: number; keyboard: boolean } | null>(null);
  const presetButton = useRef<HTMLButtonElement>(null);
  const running = job?.status === "running";

  // Options start from the project each time the dialog opens; preset, resolution and quality are remembered.
  useEffect(() => {
    const canvas = useEditor.getState().snap?.project.canvas;
    if (open && canvas) setOptions(loadOptions(canvas));
    if (open) setWhat(useCover.getState().open ? "cover" : "video");
  }, [open]);

  const close = () => {
    setError(null);
    setExists(null);
    setCreditsExist(null);
    // A finished job is shown once; a running one keeps reporting in the top bar.
    if (job && job.status !== "running") useEditor.setState({ exportJobId: null });
    const cover = useCover.getState().exportJob;
    if (cover && useEditor.getState().jobs[cover]?.status !== "running") useCover.setState({ exportJob: null });
    useEditor.setState({ exportOpen: false });
  };

  const ready = open && options !== null;
  useEffect(() => {
    if (!ready) return;
    // Closing returns focus to what opened the dialog: Export, the top bar job or a toast.
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
    return () => opener?.focus();
  }, [ready]);

  useEffect(() => {
    if (!open) return;
    // Esc closes the dialog; while an export runs, Cancel export and Keep editing say what happens to it.
    const onKey = (e: KeyboardEvent) => {
      // The preset menu closes on its own Esc, and the dialog stays.
      if (e.key !== "Escape" || presetMenu) return;
      e.stopPropagation();
      if (!running || what === "cover") close();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }); // re-bound each render so `close` sees the current job

  const jobFailed = job?.status === "failed" ? job.message : null;
  // A file that appeared at the destination while rendering is asked about like one that was there before.
  const appeared = !exists && jobFailed?.startsWith("OUTPUT_EXISTS") && last.current ? last.current.path : null;
  const replacing = exists ?? appeared;
  const asking = creditsExist ?? (replacing && fileName(replacing));
  // The question replaces the buttons that were focused, so focus moves to its safe answer.
  useEffect(() => {
    if (asking) dialog.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, [asking]);

  if (!open || !project || !options) return null;
  const duration = projectDuration(project);
  const size = outputSize(project.canvas, options.resolution);
  const setOption = (patch: Partial<ExportRequest>) => {
    const next = { ...options, ...patch };
    // Another resolution or a frame rate the preset fixes is no longer the preset; quality may change within it.
    const preset = next.preset && presetOf(next.preset);
    if (preset && (next.resolution !== preset.resolution || (preset.fps !== null && next.fps !== preset.fps))) next.preset = null;
    setOptions(next);
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ resolution: next.resolution, quality: next.quality, preset: next.preset ?? null }));
  };
  const blocked = blockedReason(options.preset, project.canvas);
  const presetItem = (p: (typeof PRESETS)[number]): MenuEntry => {
    const reason = blockedReason(p.id, project.canvas);
    const size = outputSize({ ...project.canvas, width: p.ratio[0], height: p.ratio[1] }, p.resolution);
    return {
      label: p.label,
      icon: <FormatShape width={p.ratio[0]} height={p.ratio[1]} size={14} />,
      shortcut: reason ? `Needs ${p.ratio.join(":")}` : `${size.w}×${size.h}${p.fps ? ` · ${p.fps} fps` : ""}`,
      checked: options.preset === p.id,
      disabled: reason,
      run: () => options.preset !== p.id && setOption(presetOptions(p.id, project.canvas)),
    };
  };
  const group = (items: MenuEntry[]): MenuEntry[] => (items.length ? ["separator", ...items] : []);
  const presetItems: MenuEntry[] = [
    { label: "Custom", checked: !options.preset, run: () => setOption({ preset: null }) },
    ...group(PRESETS.filter((p) => fits(p.id, project.canvas)).map(presetItem)),
    ...group(PRESETS.filter((p) => !fits(p.id, project.canvas)).map(presetItem)),
  ];

  /**
   * `epoch`: the project the export was asked for, taken before any wait. `replace`: overwriting the file was confirmed;
   * `credits`: overwriting the credits file beside it was, which is asked on its own.
   */
  const run = async (path: string, replace: boolean, credits = false, epoch = currentEpoch()) => {
    setError(null);
    setExists(null);
    setCreditsExist(null);
    last.current = { path, replace, credits };
    try {
      startedAt.current = Date.now();
      useEditor.setState({ exportJobId: await api.startExport(path, options, epoch, replace, credits) });
    } catch (e) {
      const text = errorText(e);
      if (text.startsWith("DESTINATION_EXISTS")) setExists(path);
      else if (text.startsWith("CREDITS_EXIST")) setCreditsExist(/^CREDITS_EXIST: (.+) already exists/.exec(text)?.[1] ?? "The credits file");
      else setError(text);
    }
  };

  const pickAndRun = async () => {
    const epoch = currentEpoch();
    const safe = project.name.replace(/[\\/:*?"<>|]+/g, "-").trim() || "Nuzky export";
    let defaultPath = `${safe}.mp4`;
    try {
      defaultPath = await join(await videoDir(), defaultPath);
    } catch {
      /* no Videos folder; the dialog falls back to its default location */
    }
    const picked = await save({ defaultPath, filters: [{ name: "MP4 video", extensions: ["mp4"] }] });
    if (!picked) return;
    const path = picked.endsWith(".mp4") ? picked : `${picked}.mp4`;
    // The save dialog asked before replacing the name it returned, not one with ".mp4" added.
    await run(path, path === picked, false, epoch);
  };

  const eta = (() => {
    if (!running || !job || job.progress < 0.03 || !startedAt.current) return null;
    const elapsed = (Date.now() - startedAt.current) / 1000;
    const left = (elapsed / job.progress) * (1 - job.progress);
    return left < 60 ? `${Math.ceil(left)} s left` : `${Math.ceil(left / 60)} min left`;
  })();
  const failed = asking ? null : (error ?? jobFailed);
  const settingsLocked = running || job?.status === "done";

  return (
    <div
      data-scrim
      className="fixed inset-0 z-[100] flex items-center justify-center scrim-in bg-black/45"
      onPointerDown={(e) => e.target === e.currentTarget && close()}
    >
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby="export-title"
        onKeyDown={trapTab}
        className="overlay w-[500px] rounded-[20px] p-6 dialog-in"
      >
        <div className="mb-5 flex items-start justify-between gap-3">
          <div className="flex min-w-0 flex-col gap-1">
            <h2 id="export-title" className="text-[17px] font-semibold leading-[22px] tracking-[-0.025em]">
              {what === "video" ? "Export video" : "Export cover"}
            </h2>
            <p className="tabular text-[12px] text-muted">
              {what === "video" ? `MP4 · H.264 + AAC · ${formatLabel(project.canvas.width, project.canvas.height)} · ${formatTime(duration)}` : coverFacts}
            </p>
          </div>
          <IconButton label={running ? "Hide (export keeps running)" : "Close"} round className="-mr-2 -mt-1" onClick={close}>
            <X size={16} />
          </IconButton>
        </div>

        <div className="mb-4 grid grid-cols-[104px_minmax(0,1fr)] items-center gap-x-3.5">
          <span className="text-right text-[13px] text-muted">Export</span>
          <Segmented
            label="What to export"
            value={what}
            onChange={setWhat}
            options={[
              { id: "video", label: "Video" },
              { id: "cover", label: "Cover" },
            ]}
          />
        </div>

        {what === "cover" ? (
          <CoverExport close={close} onFacts={setCoverFacts} />
        ) : (
        <>
        <div className="flex flex-col gap-4">
          <fieldset disabled={settingsLocked} className="min-w-0 disabled:opacity-50">
            <div className="grid grid-cols-[104px_minmax(0,1fr)] items-center gap-x-3.5 gap-y-3">
              <span className="text-right text-[13px] text-muted">Preset</span>
              <button
                ref={presetButton}
                type="button"
                data-menu
                aria-haspopup="menu"
                aria-expanded={!!presetMenu}
                aria-label={`Preset: ${options.preset ? presetOf(options.preset).label : "Custom"}`}
                onClick={(e) => {
                  const box = e.currentTarget.getBoundingClientRect();
                  // A click has a pointer position; Enter and Space have none.
                  setPresetMenu(presetMenu ? null : { x: box.left, y: box.bottom + 4, width: box.width, keyboard: e.detail === 0 });
                }}
                className="flex h-8 min-w-0 items-center gap-2 rounded-lg bg-white/[.09] px-2.5 text-left text-[13px] text-fg shadow-[inset_0_0_0_1px_rgb(255_255_255/.06),inset_0_1px_0_rgb(255_255_255/.06)] transition-colors duration-[120ms] ease-out enabled:hover:bg-white/[.13]"
              >
                <span className="min-w-0 flex-1 truncate">{options.preset ? presetOf(options.preset).label : "Custom"}</span>
                <ChevronDown size={14} className="shrink-0 text-muted" />
              </button>
              {presetMenu && (
                <Menu
                  items={presetItems}
                  label="Preset"
                  at={presetMenu}
                  keyboard={presetMenu.keyboard}
                  minWidth={presetMenu.width}
                  onClose={() => {
                    setPresetMenu(null);
                    presetButton.current?.focus();
                  }}
                />
              )}
              {options.preset &&
                (blocked ? (
                  <p className="col-start-2 -mt-1 flex items-start gap-1.5 text-[12px] text-fg" role="alert">
                    <AlertTriangle size={14} className="mt-px shrink-0 text-warn" />
                    {blocked}
                  </p>
                ) : (
                  <p className="tabular col-start-2 -mt-1 flex items-center gap-1.5 text-[12px] text-muted">
                    <AudioLines size={14} className="shrink-0" />
                    Loudness −14 LUFS, applied to the file
                  </p>
                ))}
              <span className="text-right text-[13px] text-muted">Resolution</span>
              <div className="flex min-w-0 items-center gap-3">
                <div className="min-w-0 flex-1">
                  <Segmented label="Resolution" value={options.resolution} onChange={(resolution) => setOption({ resolution })} options={RESOLUTIONS} />
                </div>
                <span className="tabular shrink-0 text-[12px] text-muted">
                  {size.w}×{size.h}
                </span>
              </div>
              <span className="text-right text-[13px] text-muted">Frame rate</span>
              <Segmented
                label="Frame rate"
                value={options.fps}
                onChange={(fps) => setOption({ fps })}
                options={FRAME_RATES.map((f) => ({ id: f, label: `${f}`, title: f === project.canvas.fps ? "Project frame rate" : undefined }))}
              />
              <span className="text-right text-[13px] text-muted">Quality</span>
              <Segmented label="Quality" value={options.quality} onChange={(quality) => setOption({ quality })} options={QUALITIES} />
            </div>
          </fieldset>

          {job && running && (
            <div className="flex flex-col gap-2" role="status">
              <div className="flex justify-between text-[12px]">
                <span className="tabular text-fg">
                  {job.phase ?? "Rendering"}… {Math.round(job.progress * 100)}%
                </span>
                <span className="tabular text-muted">{eta}</span>
              </div>
              <ProgressBar value={job.progress} label={job.label} />
              <p className="text-[12px] text-muted">You can keep editing. Changes made now are not part of this export.</p>
            </div>
          )}
          {job?.status === "done" && (
            <div className="flex items-start gap-2 rounded-xl bg-ok/10 p-3 text-[13px] text-fg" role="status">
              <CheckCircle2 size={16} className="mt-px shrink-0 text-ok" />
              <span className="break-all">Saved to {job.output}</span>
            </div>
          )}
          {asking && (
            <div className="flex items-start gap-2 rounded-xl bg-warn/10 p-3 text-[13px] text-fg" role="alert">
              <AlertTriangle size={16} className="mt-px shrink-0 text-warn" />
              <span className="break-all">{asking} already exists. Replace it?</span>
            </div>
          )}
          {failed && (
            <div className="flex items-start gap-2 rounded-xl bg-danger/10 p-3 text-[13px] text-danger" role="alert">
              <AlertCircle size={16} className="mt-px shrink-0" />
              <span>Export failed: {plainError(failed)}</span>
            </div>
          )}
          {job?.status === "cancelled" && <p className="text-[12px] text-muted">Export cancelled. The partial file was removed.</p>}
        </div>

        <div className="mt-6 flex items-center justify-between gap-3 border-t border-white/[.08] pt-4">
          <span className={`tabular min-w-0 text-[13px] text-muted ${settingsLocked ? "opacity-50" : ""}`} title="Estimate; scenes with a lot of motion come out larger">
            about <span className="font-semibold text-fg">{formatBytes(estimateBytes(size.w, size.h, options.fps, options.quality, duration))}</span>
          </span>
          <div className="flex shrink-0 gap-2.5">
            {running ? (
              <>
                <Button pill onClick={() => api.cancelJob(jobId!)}>
                  Cancel export
                </Button>
                <Button pill variant="primary" data-autofocus onClick={close}>
                  Keep editing
                </Button>
              </>
            ) : job?.status === "done" ? (
              <>
                <Button pill onClick={() => job.output && revealItemInDir(job.output)}>
                  Show in folder
                </Button>
                <Button pill variant="primary" data-autofocus onClick={close}>
                  Done
                </Button>
              </>
            ) : asking ? (
              <>
                <Button pill data-autofocus onClick={() => (setExists(null), setCreditsExist(null), pickAndRun())}>
                  Choose another name…
                </Button>
                <Button
                  pill
                  variant="danger"
                  onClick={() => (creditsExist ? run(last.current!.path, last.current!.replace, true) : run(replacing!, true, last.current?.credits))}
                >
                  Replace
                </Button>
              </>
            ) : (
              <>
                <Button pill onClick={close}>
                  Close
                </Button>
                {failed && last.current ? (
                  <Button pill variant="primary" data-autofocus disabled={!!blocked} disabledReason={blocked ?? undefined} onClick={() => run(last.current!.path, last.current!.replace, last.current!.credits)}>
                    Retry
                  </Button>
                ) : (
                  <Button pill variant="primary" data-autofocus disabled={!!blocked} disabledReason={blocked ?? undefined} onClick={pickAndRun}>
                    Export…
                  </Button>
                )}
              </>
            )}
          </div>
        </div>
        </>
        )}
      </div>
    </div>
  );
}
