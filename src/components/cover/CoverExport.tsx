import { useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { join, pictureDir } from "@tauri-apps/api/path";
import { AlertCircle, AlertTriangle, CheckCircle2 } from "lucide-react";
import { api, errorText, plainError } from "../../lib/api";
import { COVER_FORMATS, coverFormat, openCover, thumbnailOf, useCover } from "../../lib/cover";
import { currentEpoch, useEditor } from "../../lib/store";
import type { ThumbnailFormat } from "../../lib/types";
import { Button, ProgressBar, Segmented } from "../ui";

type Kind = "png" | "jpg";
const KIND_KEY = "nuzky.coverExport";
const fileName = (path: string) => path.split(/[\\/]/).pop() || path;

/** PNG for the cover; JPEG for YouTube, which takes thumbnails up to 2 MB. Remembered per format. */
function loadKind(format: ThumbnailFormat): Kind {
  try {
    const saved = (JSON.parse(localStorage.getItem(KIND_KEY) ?? "{}") as Partial<Record<ThumbnailFormat, Kind>>)[format];
    if (saved === "png" || saved === "jpg") return saved;
  } catch {
    /* a damaged setting falls back to the default */
  }
  return format === "cover_9x16" ? "png" : "jpg";
}

function saveKind(format: ThumbnailFormat, kind: Kind) {
  try {
    localStorage.setItem(KIND_KEY, JSON.stringify({ ...(JSON.parse(localStorage.getItem(KIND_KEY) ?? "{}") as object), [format]: kind }));
  } catch {
    localStorage.setItem(KIND_KEY, JSON.stringify({ [format]: kind }));
  }
}

/** The Cover side of the Export dialog: the format and file type, then a job that writes the image. */
export function CoverExport({ close }: { close: () => void }) {
  const project = useEditor((s) => s.snap?.project);
  const [format, setFormat] = useState<ThumbnailFormat>(() => useCover.getState().open ?? (thumbnailOf(project, "cover_9x16") || !thumbnailOf(project, "youtube_16x9") ? "cover_9x16" : "youtube_16x9"));
  const [kind, setKind] = useState<Kind>(() => loadKind(format));
  const jobId = useCover((s) => s.exportJob);
  const job = useEditor((s) => (jobId ? s.jobs[jobId] : undefined));
  const [error, setError] = useState<string | null>(null);
  /** The file asked for already exists; Replace exports over it. */
  const [exists, setExists] = useState<string | null>(null);
  const last = useRef<{ path: string; replace: boolean } | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const running = job?.status === "running";
  const done = job?.status === "done";
  const cover = thumbnailOf(project, format);
  const f = coverFormat(format);

  // The question replaces the buttons that were focused, so focus moves to its safe answer.
  useEffect(() => {
    if (exists) root.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, [exists]);

  const run = async (path: string, replace: boolean, epoch = currentEpoch()) => {
    setError(null);
    setExists(null);
    last.current = { path, replace };
    try {
      useCover.setState({ exportJob: await api.startCoverExport(format, path, replace, epoch) });
    } catch (e) {
      const text = errorText(e);
      if (text.startsWith("DESTINATION_EXISTS")) setExists(path);
      else setError(text);
    }
  };

  const pickAndRun = async () => {
    const epoch = currentEpoch();
    const safe = (project?.name ?? "").replace(/[\\/:*?"<>|]+/g, "-").trim() || "Nuzky";
    let defaultPath = `${safe} ${format === "cover_9x16" ? "cover" : "YouTube thumbnail"}.${kind}`;
    try {
      defaultPath = await join(await pictureDir(), defaultPath);
    } catch {
      /* no Pictures folder; the dialog falls back to its default location */
    }
    const picked = await save({ defaultPath, filters: [kind === "png" ? { name: "PNG image", extensions: ["png"] } : { name: "JPEG image", extensions: ["jpg", "jpeg"] }] });
    if (!picked) return;
    const path = /\.(png|jpe?g)$/i.test(picked) ? picked : `${picked}.${kind}`;
    // The save dialog asked before replacing the name it returned, not one with the extension added.
    await run(path, path === picked, epoch);
  };

  const failed = exists ? null : (error ?? (job?.status === "failed" ? job.message : null));
  const appeared = !exists && failed?.startsWith("OUTPUT_EXISTS") && last.current ? last.current.path : null;
  const replacing = exists ?? appeared;
  const locked = running || done;

  return (
    <div ref={root} className="flex flex-col">
      <fieldset disabled={locked} className="min-w-0 disabled:opacity-50">
        <div className="grid grid-cols-[104px_minmax(0,1fr)] items-center gap-x-3.5 gap-y-3">
          <span className="text-right text-[13px] text-muted">Format</span>
          <div className="flex min-w-0 items-center gap-3">
            <div className="min-w-0 flex-1">
              <Segmented
                label="Cover format"
                value={format}
                onChange={(id) => {
                  setFormat(id);
                  setKind(loadKind(id));
                }}
                options={COVER_FORMATS.map((c) => ({ id: c.id, label: c.label }))}
              />
            </div>
            <span className="tabular shrink-0 text-[12px] text-muted">
              {f.width}×{f.height}
            </span>
          </div>
          <span className="text-right text-[13px] text-muted">File</span>
          <Segmented
            label="File type"
            value={kind}
            onChange={(id) => {
              setKind(id);
              saveKind(format, id);
            }}
            options={[
              { id: "png", label: "PNG" },
              { id: "jpg", label: "JPEG", title: "Under 2 MB, as YouTube needs" },
            ]}
          />
        </div>
      </fieldset>

      <div className="mt-4 flex flex-col gap-4 empty:hidden">
        {!cover && (
          <p className="flex items-start gap-1.5 text-[12px] text-fg" role="status">
            <AlertTriangle size={14} className="mt-px shrink-0 text-warn" />
            There is no {f.noun} yet. Make it in the cover editor first.
          </p>
        )}
        {job && running && (
          <div className="flex flex-col gap-2" role="status">
            <span className="tabular text-[12px] text-fg">
              {job.phase ?? "Rendering"}… {job.progress > 0 ? `${Math.round(job.progress * 100)}%` : ""}
            </span>
            <ProgressBar value={job.progress} label={job.label} />
          </div>
        )}
        {done && (
          <div className="flex items-start gap-2 rounded-xl bg-ok/10 p-3 text-[13px] text-fg" role="status">
            <CheckCircle2 size={16} className="mt-px shrink-0 text-ok" />
            <span className="break-all">Saved to {job.output}</span>
          </div>
        )}
        {replacing && (
          <div className="flex items-start gap-2 rounded-xl bg-warn/10 p-3 text-[13px] text-fg" role="alert">
            <AlertTriangle size={16} className="mt-px shrink-0 text-warn" />
            <span className="break-all">{fileName(replacing)} already exists. Replace it?</span>
          </div>
        )}
        {failed && (
          <div className="flex items-start gap-2 rounded-xl bg-danger/10 p-3 text-[13px] text-danger" role="alert">
            <AlertCircle size={16} className="mt-px shrink-0" />
            <span>Export failed: {plainError(failed)}</span>
          </div>
        )}
        {job?.status === "cancelled" && <p className="text-[12px] text-muted">Export cancelled. Nothing was written.</p>}
      </div>

      <div className="mt-6 flex items-center justify-end gap-2.5 border-t border-white/[.08] pt-4">
        {running ? (
          <>
            <Button pill onClick={() => void api.cancelJob(jobId!)}>
              Cancel export
            </Button>
            <Button pill variant="primary" data-autofocus onClick={close}>
              Keep editing
            </Button>
          </>
        ) : done ? (
          <>
            <Button pill onClick={() => job.output && revealItemInDir(job.output)}>
              Show in folder
            </Button>
            <Button pill variant="primary" data-autofocus onClick={close}>
              Done
            </Button>
          </>
        ) : replacing ? (
          <>
            <Button pill data-autofocus onClick={() => (setExists(null), pickAndRun())}>
              Choose another name…
            </Button>
            <Button pill variant="danger" onClick={() => run(replacing, true)}>
              Replace
            </Button>
          </>
        ) : !cover ? (
          <>
            <Button pill onClick={close}>
              Close
            </Button>
            <Button
              pill
              variant="primary"
              data-autofocus
              onClick={() => {
                close();
                openCover(format);
              }}
            >
              Make the {format === "cover_9x16" ? "cover" : "thumbnail"}
            </Button>
          </>
        ) : (
          <>
            <Button pill onClick={close}>
              Close
            </Button>
            {failed && last.current ? (
              <Button pill variant="primary" data-autofocus onClick={() => run(last.current!.path, last.current!.replace)}>
                Retry
              </Button>
            ) : (
              <Button pill variant="primary" data-autofocus onClick={pickAndRun}>
                Export…
              </Button>
            )}
          </>
        )}
      </div>
    </div>
  );
}
