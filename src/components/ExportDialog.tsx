import { useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { videoDir, join } from "@tauri-apps/api/path";
import { AlertCircle, CheckCircle2, Download, FolderOpen, X } from "lucide-react";
import { api, errorText } from "../lib/api";
import { formatLabel } from "../lib/presets";
import { currentEpoch, projectDuration, useEditor } from "../lib/store";
import { US, formatTime } from "../lib/time";
import type { Canvas, ExportRequest } from "../lib/types";
import { Button, IconButton, ProgressBar, Segmented } from "./ui";

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
const STORAGE_KEY = "capopen.export";

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

function formatBytes(b: number) {
  if (b >= 1e9) return `${(b / 1e9).toFixed(1)} GB`;
  return `${Math.max(1, Math.round(b / 1e6))} MB`;
}

function loadOptions(canvas: Canvas): ExportRequest {
  const short = Math.min(canvas.width, canvas.height);
  const fallback: ExportRequest = {
    resolution: RESOLUTIONS.some((r) => r.id === short) ? short : 1080,
    fps: FRAME_RATES.includes(canvas.fps) ? canvas.fps : 30,
    quality: "recommended",
  };
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null") as Partial<ExportRequest> | null;
    return { ...fallback, ...(saved?.resolution && { resolution: saved.resolution }), ...(saved?.quality && { quality: saved.quality }) };
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
  const [error, setError] = useState<string | null>(null);
  const lastPath = useRef<string | null>(null);
  const startedAt = useRef(0);
  const dialog = useRef<HTMLDivElement>(null);
  const running = job?.status === "running";

  // Options start from the project each time the dialog opens; resolution and quality are remembered.
  useEffect(() => {
    const canvas = useEditor.getState().snap?.project.canvas;
    if (open && canvas) setOptions(loadOptions(canvas));
  }, [open]);

  const close = () => {
    setError(null);
    // A finished job is shown once; a running one keeps reporting in the top bar.
    if (job && job.status !== "running") useEditor.setState({ exportJobId: null });
    useEditor.setState({ exportOpen: false });
  };

  const ready = open && options !== null;
  useEffect(() => {
    if (ready) dialog.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, [ready]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      close();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }); // re-bound each render so `close` sees the current job

  if (!open || !project || !options) return null;
  const duration = projectDuration(project);
  const size = outputSize(project.canvas, options.resolution);
  const setOption = (patch: Partial<ExportRequest>) => {
    const next = { ...options, ...patch };
    setOptions(next);
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ resolution: next.resolution, quality: next.quality }));
  };

  const run = async (path: string) => {
    setError(null);
    lastPath.current = path;
    try {
      startedAt.current = Date.now();
      useEditor.setState({ exportJobId: await api.startExport(path, options, currentEpoch()) });
    } catch (e) {
      setError(errorText(e));
    }
  };

  const pickAndRun = async () => {
    const safe = project.name.replace(/[\\/:*?"<>|]+/g, "-").trim() || "CapOpen export";
    let defaultPath = `${safe}.mp4`;
    try {
      defaultPath = await join(await videoDir(), defaultPath);
    } catch {
      /* no Videos folder; the dialog falls back to its default location */
    }
    const path = await save({ defaultPath, filters: [{ name: "MP4 video", extensions: ["mp4"] }] });
    if (path) await run(path.endsWith(".mp4") ? path : `${path}.mp4`);
  };

  const eta = (() => {
    if (!running || !job || job.progress < 0.03 || !startedAt.current) return null;
    const elapsed = (Date.now() - startedAt.current) / 1000;
    const left = (elapsed / job.progress) * (1 - job.progress);
    return left < 60 ? `${Math.ceil(left)} s left` : `${Math.ceil(left / 60)} min left`;
  })();
  const failed = error ?? (job?.status === "failed" ? job.message : null);
  const settingsLocked = running || job?.status === "done";

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/60" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div ref={dialog} role="dialog" aria-modal="true" aria-labelledby="export-title" className="w-[460px] rounded-lg border border-line bg-panel shadow-2xl shadow-black">
        <div className="flex items-center justify-between border-b border-line px-4 py-3">
          <h2 id="export-title" className="text-[14px] font-semibold">
            Export video
          </h2>
          <IconButton label={running ? "Hide (export keeps running)" : "Close"} onClick={close}>
            <X size={16} />
          </IconButton>
        </div>

        <div className="flex flex-col gap-4 p-4">
          <fieldset disabled={settingsLocked} className="flex flex-col gap-4 disabled:opacity-50">
            <div className="flex flex-col gap-1.5">
              <div className="flex items-baseline justify-between">
                <span className="text-[12px] text-muted">Resolution</span>
                <span className="tabular text-[12px] text-fg">
                  {size.w}×{size.h}
                </span>
              </div>
              <Segmented label="Resolution" value={options.resolution} onChange={(resolution) => setOption({ resolution })} options={RESOLUTIONS} />
            </div>
            <div className="flex flex-col gap-1.5">
              <span className="text-[12px] text-muted">Frame rate</span>
              <Segmented
                label="Frame rate"
                value={options.fps}
                onChange={(fps) => setOption({ fps })}
                options={FRAME_RATES.map((f) => ({ id: f, label: `${f}`, title: f === project.canvas.fps ? "Project frame rate" : undefined }))}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <div className="flex items-baseline justify-between">
                <span className="text-[12px] text-muted">Quality</span>
                <span className="tabular text-[12px] text-fg" title="Estimate; scenes with a lot of motion come out larger">
                  about {formatBytes(estimateBytes(size.w, size.h, options.fps, options.quality, duration))}
                </span>
              </div>
              <Segmented label="Quality" value={options.quality} onChange={(quality) => setOption({ quality })} options={QUALITIES} />
            </div>
          </fieldset>
          <p className="tabular text-[12px] text-muted">
            MP4 · H.264 + AAC · {formatLabel(project.canvas.width, project.canvas.height)} · {formatTime(duration)}
          </p>

          {job && running && (
            <div className="flex flex-col gap-2" role="status">
              <div className="flex justify-between text-[12px]">
                <span className="text-fg">Rendering… {Math.round(job.progress * 100)}%</span>
                <span className="tabular text-muted">{eta}</span>
              </div>
              <ProgressBar value={job.progress} label={job.label} />
              <p className="text-[12px] text-muted">You can keep editing. Changes made now are not part of this export.</p>
            </div>
          )}
          {job?.status === "done" && (
            <div className="flex items-start gap-2 rounded-md bg-accent/10 p-3 text-[13px]" role="status">
              <CheckCircle2 size={16} className="mt-px shrink-0 text-accent" />
              <span className="break-all">Saved to {job.output}</span>
            </div>
          )}
          {failed && (
            <div className="flex items-start gap-2 rounded-md bg-danger/10 p-3 text-[13px] text-danger" role="alert">
              <AlertCircle size={16} className="mt-px shrink-0" />
              <span>Export failed: {failed}</span>
            </div>
          )}
          {job?.status === "cancelled" && <p className="text-[12px] text-muted">Export cancelled. The partial file was removed.</p>}
        </div>

        <div className="flex justify-end gap-2 border-t border-line px-4 py-3">
          {running ? (
            <>
              <Button onClick={() => api.cancelJob(jobId!)}>Cancel export</Button>
              <Button variant="primary" data-autofocus onClick={close}>
                Keep editing
              </Button>
            </>
          ) : job?.status === "done" ? (
            <>
              <Button onClick={() => job.output && revealItemInDir(job.output)}>
                <FolderOpen size={15} /> Show in folder
              </Button>
              <Button variant="primary" data-autofocus onClick={close}>
                Done
              </Button>
            </>
          ) : (
            <>
              <Button onClick={close}>Close</Button>
              {failed && lastPath.current ? (
                <Button variant="primary" data-autofocus onClick={() => run(lastPath.current!)}>
                  <Download size={15} /> Retry
                </Button>
              ) : (
                <Button variant="primary" data-autofocus onClick={pickAndRun}>
                  <Download size={15} /> Export…
                </Button>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
