import { useEffect, useRef } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, ArrowRight, Check, Film, Plus, X } from "lucide-react";
import { api, plainError } from "../../lib/api";
import { NO_PAIR, fileName, startLearning, useStyle } from "../../lib/style";
import { useEditor } from "../../lib/store";
import type { StylePair } from "../../lib/types";
import { Button, IconButton, ProgressBar, trapTab } from "../ui";

const VIDEO_EXTENSIONS = ["mp4", "mov", "m4v", "mkv", "webm", "avi", "mts"];

async function pickVideo(): Promise<string | null> {
  const picked = await open({ multiple: false, filters: [{ name: "Video", extensions: VIDEO_EXTENSIONS }] });
  return typeof picked === "string" ? picked : null;
}

function FileButton({ path, label, disabled, onPick }: { path: string; label: string; disabled: boolean; onPick: (path: string) => void }) {
  return (
    <Button
      className="h-8 min-w-0 flex-1 justify-start gap-2 px-2.5"
      aria-label={path ? `${label}: ${fileName(path)}` : `Choose the ${label.toLowerCase()}`}
      title={path || undefined}
      disabled={disabled}
      disabledReason="Learning is running"
      onClick={async () => {
        const picked = await pickVideo();
        if (picked) onPick(picked);
      }}
    >
      <Film size={14} className="shrink-0 text-muted" />
      <span className={`truncate ${path ? "text-fg" : "text-muted"}`}>{path ? fileName(path) : "Choose…"}</span>
    </Button>
  );
}

/** Learns the style from 1 to 3 recordings, each with the finished video cut from it. */
export function LearnDialog() {
  const learning = useStyle((s) => s.learning);
  const job = useEditor((s) => (learning ? s.jobs[learning.jobId] : undefined));
  const running = job?.status === "running";
  const ended = !!learning && !!job && !running;
  const rows = useStyle((s) => s.draft);
  const setRows = (change: (rows: StylePair[]) => StylePair[]) => useStyle.setState((s) => ({ draft: change(s.draft) }));
  const dialog = useRef<HTMLDivElement>(null);
  const pairs = learning && (running || ended) ? learning.pairs : rows;
  const close = () => useStyle.setState({ learnOpen: false });

  useEffect(() => {
    dialog.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, []);

  const set = (i: number, side: keyof StylePair, path: string) => setRows((all) => all.map((r, k) => (k === i ? { ...r, [side]: path } : r)));
  const ready = rows.length > 0 && rows.every((r) => r.recording && r.cut);
  const runningRow = learning?.results.findIndex((r) => r === null) ?? -1;
  // The job's progress is over every pair; each row shows its own part.
  const rowProgress = job ? Math.min(1, Math.max(0, job.progress * pairs.length - runningRow)) : 0;

  const learn = () => {
    if (!ready) return;
    useStyle.setState({ learning: null });
    void startLearning(rows);
  };

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center scrim-in bg-black/45" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby="learn-title"
        onKeyDown={(e) => {
          trapTab(e);
          if (e.key === "Escape") {
            // Esc closes only this, never the home screen behind it.
            e.preventDefault();
            e.stopPropagation();
            close();
          }
        }}
        className="overlay w-[640px] max-w-[calc(100vw-48px)] rounded-[20px] p-6 dialog-in"
      >
        <div className="mb-5 flex items-start justify-between gap-3">
          <h2 id="learn-title" className="text-[17px] font-semibold leading-[22px] tracking-[-0.025em]">
            Learn from videos
          </h2>
          <IconButton label="Close" round className="-mr-2 -mt-1" onClick={close}>
            <X size={16} />
          </IconButton>
        </div>
        <div className="grid grid-cols-[1fr_16px_1fr_196px] items-center gap-x-2.5 gap-y-2">
          <span className="text-[12px] text-muted">Recording</span>
          <span />
          <span className="text-[12px] text-muted">Finished video</span>
          <span />
          {pairs.map((pair, i) => {
            const result = learning && (running || ended) ? learning.results[i] : null;
            return (
              <div key={i} role="group" aria-label={`Pair ${i + 1}`} className="contents">
                <FileButton path={pair.recording} label="Recording" disabled={running} onPick={(p) => set(i, "recording", p)} />
                <ArrowRight size={14} className="text-muted" aria-hidden />
                <FileButton path={pair.cut} label="Finished video" disabled={running} onPick={(p) => set(i, "cut", p)} />
                <div className="flex min-h-8 min-w-0 items-center text-[12px]" aria-live="polite">
                  {result?.error ? (
                    <span className="flex items-start gap-1.5 text-fg" title={result.error}>
                      <AlertTriangle size={14} className="mt-px shrink-0 text-warn" />
                      <span className="line-clamp-2">{plainError(result.error)}</span>
                    </span>
                  ) : result ? (
                    <span className="flex items-center gap-1.5 text-fg">
                      <Check size={14} className="shrink-0 text-ok" />
                      Matched {Math.round((result.matched ?? 0) * 100)}%
                    </span>
                  ) : running && i === runningRow ? (
                    <div className="flex w-full flex-col gap-1.5">
                      <span className="flex justify-between gap-2 text-fg">
                        <span className="truncate">{job?.phase ?? "Starting"}</span>
                        <span className="tabular text-muted">{Math.round(rowProgress * 100)}%</span>
                      </span>
                      <ProgressBar value={rowProgress} label={`Pair ${i + 1}`} />
                    </div>
                  ) : running ? (
                    <span className="text-muted">Waiting</span>
                  ) : !learning || !ended ? (
                    rows.length > 1 && (
                      <IconButton label={`Remove pair ${i + 1}`} onClick={() => setRows((all) => all.filter((_, k) => k !== i))}>
                        <X size={15} />
                      </IconButton>
                    )
                  ) : null}
                </div>
              </div>
            );
          })}
        </div>
        {!running && !ended && rows.length < 3 && (
          <Button className="mt-3 h-8 gap-1.5 px-2.5" onClick={() => setRows((all) => [...all, NO_PAIR])}>
            <Plus size={14} /> Add a pair
          </Button>
        )}
        {ended && job?.status === "failed" && (
          <p className="mt-4 flex items-start gap-1.5 text-[12px] text-fg" role="alert">
            <AlertTriangle size={14} className="mt-px shrink-0 text-danger" />
            {plainError(job.message ?? "Learning failed.")}
          </p>
        )}
        <div className="mt-6 flex items-center gap-2 border-t border-white/[.08] pt-4">
          <p className="flex-1 text-[12px] text-muted">
            {running ? "Keeps running if you close this." : "Each finished video is compared with its recording here, on this computer."}
          </p>
          {running ? (
            <Button pill data-autofocus onClick={() => job && void api.cancelJob(job.id)}>
              Stop
            </Button>
          ) : ended && job?.status === "done" ? (
            <>
              <Button
                pill
                onClick={() => {
                  useStyle.setState({ learning: null });
                  setRows(() => [NO_PAIR]);
                }}
              >
                Learn from more
              </Button>
              <Button pill variant="primary" data-autofocus onClick={close}>
                Review
              </Button>
            </>
          ) : (
            <Button pill variant="primary" data-autofocus disabled={!ready} disabledReason="Choose a recording and the video you cut from it" onClick={learn}>
              Learn
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
