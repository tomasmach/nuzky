import { useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { videoDir, join } from "@tauri-apps/api/path";
import { AlertCircle, CheckCircle2, Download, FolderOpen, X } from "lucide-react";
import { api, errorText } from "../lib/api";
import { projectDuration, useEditor } from "../lib/store";
import { formatTime } from "../lib/time";
import { Button, IconButton, ProgressBar } from "./ui";
import { formatLabel } from "./TopBar";

export function ExportDialog() {
  const open = useEditor((s) => s.exportOpen);
  const project = useEditor((s) => s.snap?.project);
  const jobs = useEditor((s) => s.jobs);
  const [jobId, setJobId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLDivElement>(null);
  const job = jobId ? jobs[jobId] : undefined;
  const running = job?.status === "running";
  const startedAt = useRef(0);

  const close = () => {
    if (running) return;
    setJobId(null);
    setError(null);
    useEditor.setState({ exportOpen: false });
  };

  useEffect(() => {
    if (!open) return;
    dialog.current?.querySelector<HTMLElement>("button[data-autofocus]")?.focus();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (!open || !project) return null;
  const duration = projectDuration(project);

  const start = async () => {
    setError(null);
    const safe = project.name.replace(/[\\/:*?"<>|]+/g, "-").trim() || "CapOpen export";
    let defaultPath = `${safe}.mp4`;
    try {
      defaultPath = await join(await videoDir(), defaultPath);
    } catch {
      /* no Videos folder; the dialog falls back to its default location */
    }
    const path = await save({ defaultPath, filters: [{ name: "MP4 video", extensions: ["mp4"] }] });
    if (!path) return;
    try {
      startedAt.current = Date.now();
      setJobId(await api.startExport(path.endsWith(".mp4") ? path : `${path}.mp4`));
    } catch (e) {
      setError(errorText(e));
    }
  };

  const eta = (() => {
    if (!running || !job || job.progress < 0.03) return null;
    const elapsed = (Date.now() - startedAt.current) / 1000;
    const left = (elapsed / job.progress) * (1 - job.progress);
    return left < 60 ? `${Math.ceil(left)} s left` : `${Math.ceil(left / 60)} min left`;
  })();

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/60" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div ref={dialog} role="dialog" aria-modal="true" aria-labelledby="export-title" className="w-[420px] rounded-lg border border-line bg-panel shadow-2xl shadow-black">
        <div className="flex items-center justify-between border-b border-line px-4 py-3">
          <h2 id="export-title" className="text-[14px] font-semibold">
            Export video
          </h2>
          <IconButton label="Close" onClick={close} disabled={running}>
            <X size={16} />
          </IconButton>
        </div>

        <div className="flex flex-col gap-4 p-4">
          <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 text-[13px]">
            <dt className="text-muted">Format</dt>
            <dd>
              MP4 · H.264 + AAC · {formatLabel(project.canvas.width, project.canvas.height)}
            </dd>
            <dt className="text-muted">Resolution</dt>
            <dd className="tabular">
              {project.canvas.width}×{project.canvas.height} · {project.canvas.fps} fps
            </dd>
            <dt className="text-muted">Length</dt>
            <dd className="tabular">{formatTime(duration)}</dd>
          </dl>

          {job && running && (
            <div className="flex flex-col gap-2" role="status">
              <div className="flex justify-between text-[12px]">
                <span className="text-fg">Rendering… {Math.round(job.progress * 100)}%</span>
                <span className="tabular text-muted">{eta}</span>
              </div>
              <ProgressBar value={job.progress} />
              <p className="text-[12px] text-subtle">You can keep editing. Changes made now are not part of this export.</p>
            </div>
          )}
          {job?.status === "done" && (
            <div className="flex items-start gap-2 rounded-md bg-accent/10 p-3 text-[13px]" role="status">
              <CheckCircle2 size={16} className="mt-px shrink-0 text-accent" />
              <span className="break-all">Saved to {job.output}</span>
            </div>
          )}
          {(error || job?.status === "failed") && (
            <div className="flex items-start gap-2 rounded-md bg-danger/10 p-3 text-[13px] text-danger" role="alert">
              <AlertCircle size={16} className="mt-px shrink-0" />
              <span>Export failed: {error ?? job?.message}</span>
            </div>
          )}
          {job?.status === "cancelled" && <p className="text-[12px] text-muted">Export cancelled. The partial file was removed.</p>}
        </div>

        <div className="flex justify-end gap-2 border-t border-line px-4 py-3">
          {running ? (
            <Button onClick={() => job && api.cancelJob(job.id)}>Cancel export</Button>
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
              <Button variant="primary" data-autofocus onClick={start}>
                <Download size={15} /> {error || job?.status === "failed" ? "Retry" : "Export…"}
              </Button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
