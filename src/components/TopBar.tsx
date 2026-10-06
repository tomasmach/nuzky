import { useEffect, useRef, useState } from "react";
import { AlertCircle, Check, Download, FilePlus2, FolderOpen, Loader2, Redo2, Sparkles, Undo2 } from "lucide-react";
import { api, errorText } from "../lib/api";
import { FORMATS } from "../lib/presets";
import { openExport, projectDuration, stopAiRun, useEditor } from "../lib/store";
import { formatDuration } from "../lib/time";
import type { ProjectSummary, Snapshot } from "../lib/types";
import { Button, IconButton, ProgressBar } from "./ui";

function SaveStatus() {
  const saveState = useEditor((s) => s.saveState);
  if (saveState === "error")
    return (
      <span className="flex items-center gap-1 text-[12px] text-danger" role="status">
        <AlertCircle size={14} /> Not saved, retrying on next edit
      </span>
    );
  if (saveState === "saving")
    return (
      <span className="flex items-center gap-1 text-[12px] text-muted" role="status">
        <Loader2 size={13} className="animate-spin" /> Saving…
      </span>
    );
  return (
    <span className="flex items-center gap-1 text-[12px] text-muted" role="status">
      <Check size={13} /> Saved
    </span>
  );
}

function ProjectName() {
  const name = useEditor((s) => s.snap?.project.name ?? "");
  const edit = useEditor((s) => s.edit);
  const [draft, setDraft] = useState<string | null>(null);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (draft !== null) ref.current?.select();
  }, [draft !== null]); // eslint-disable-line react-hooks/exhaustive-deps

  if (draft === null)
    return (
      <button
        type="button"
        title="Rename project"
        onClick={() => setDraft(name)}
        className="max-w-[260px] truncate rounded px-1.5 py-0.5 text-[13px] font-medium text-fg hover:bg-raised"
      >
        {name}
      </button>
    );
  const commit = () => {
    if (draft.trim() && draft !== name) edit({ type: "renameProject", name: draft });
    setDraft(null);
  };
  return (
    <input
      ref={ref}
      aria-label="Project name"
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") setDraft(null);
      }}
      className="h-7 w-[260px] rounded border border-accent bg-raised px-1.5 text-[13px] text-fg"
    />
  );
}

function ProjectMenu() {
  const [open, setOpen] = useState(false);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const current = useEditor((s) => s.snap?.path);
  const { setSnap, toast } = useEditor.getState();
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    api.listProjects().then(setProjects);
    const close = (e: PointerEvent) => !ref.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  const run = async (fn: () => Promise<Snapshot>) => {
    setOpen(false);
    try {
      setSnap(await fn(), false, true);
      useEditor.setState({ timeUs: 0, thumbs: {}, filmstrips: {}, waveforms: {} });
    } catch (e) {
      toast({ kind: "error", text: errorText(e) });
    }
  };

  return (
    <div className="relative" ref={ref}>
      <IconButton label="Projects" onClick={() => setOpen(!open)} active={open}>
        <FolderOpen size={16} />
      </IconButton>
      {open && (
        <div className="absolute left-0 top-10 z-50 w-80 rounded-lg border border-line bg-panel p-2 shadow-2xl shadow-black/60">
          <div className="px-2 pb-1 pt-1 text-[11px] font-semibold uppercase tracking-wide text-muted">New project</div>
          <div className="grid grid-cols-4 gap-1 pb-2">
            {FORMATS.map((f) => (
              <button
                key={f.label}
                type="button"
                title={f.hint}
                onClick={() => run(() => api.newProject(f.width, f.height))}
                className="flex flex-col items-center gap-1 rounded-md px-1 py-2 text-[12px] text-muted hover:bg-raised hover:text-fg"
              >
                <FilePlus2 size={16} />
                {f.label}
              </button>
            ))}
          </div>
          <div className="border-t border-line px-2 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wide text-muted">Recent</div>
          <div className="max-h-72 overflow-y-auto">
            {projects.length === 0 && <div className="px-2 py-2 text-[12px] text-muted">No saved projects yet.</div>}
            {projects.map((p) => (
              <button
                key={p.path}
                type="button"
                disabled={p.path === current}
                onClick={() => run(() => api.openProject(p.path))}
                className="flex w-full items-center justify-between rounded-md px-2 py-1.5 text-left hover:bg-raised disabled:cursor-default disabled:bg-raised/60"
              >
                <span className="truncate text-[13px] text-fg">{p.name}</span>
                <span className="tabular shrink-0 pl-2 text-[11px] text-muted">
                  {p.path === current ? "Open" : formatDuration(p.durationUs)}
                </span>
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

/** Background work. Export shows first and reopens its dialog; captions and transcripts open their tab. */
function JobIndicator() {
  const jobs = useEditor((s) => s.jobs);
  const running = Object.values(jobs).filter((j) => j.status === "running");
  if (running.length === 0) return null;
  const j = running.find((x) => x.kind === "export") ?? running.find((x) => x.kind === "captions" || x.kind === "transcript") ?? running[0];
  const pct = j.progress > 0 ? `${Math.round(j.progress * 100)}%` : null;
  const body = (
    <>
      <Loader2 size={13} className="shrink-0 animate-spin text-accent" />
      <span className="max-w-[220px] truncate">{j.kind === "export" ? "Exporting" : (j.phase ?? j.label)}</span>
      {pct && <span className="tabular text-muted">{pct}</span>}
      {j.kind === "export" && <ProgressBar value={j.progress} className="w-16" />}
      {running.length > 1 && <span className="text-muted">+{running.length - 1}</span>}
    </>
  );
  if (j.kind === "audio")
    return (
      <span className="flex items-center gap-1.5 text-[12px] text-fg" role="status">
        {body}
      </span>
    );
  return (
    <button
      type="button"
      role="status"
      title={j.kind === "export" ? "Show export progress" : j.kind === "transcript" ? "Show transcript" : "Show captions"}
      onClick={() => (j.kind === "export" ? useEditor.setState({ exportOpen: true }) : useEditor.setState({ panelTab: j.kind === "transcript" ? "transcript" : "captions" }))}
      className="flex h-8 items-center gap-1.5 rounded-md px-2 text-[12px] text-fg hover:bg-raised"
    >
      {body}
    </button>
  );
}

/** While an agent edits, what it is doing and a way to take over. */
function AiRunBar() {
  const label = useEditor((s) => s.aiRun);
  if (!label) return null;
  return (
    <div className="flex h-8 max-w-[420px] items-center gap-2 rounded-md bg-raised pl-2.5 pr-1 text-[12px] text-fg" role="status">
      <Sparkles size={14} className="shrink-0 animate-pulse text-accent" />
      <span className="truncate">
        AI is editing <span className="text-muted">· {label}</span>
      </span>
      <Button className="h-6 px-2 text-[12px]" title="Stop the AI and keep what it did so far" onClick={() => void stopAiRun()}>
        Stop
      </Button>
    </div>
  );
}

export function TopBar() {
  const snap = useEditor((s) => s.snap);
  const { undo, redo } = useEditor.getState();
  const aiRun = useEditor((s) => s.aiRun);
  const empty = !snap || projectDuration(snap.project) === 0;
  return (
    <header className="flex h-12 shrink-0 items-center gap-3 border-b border-line bg-panel px-3">
      <img src="/icon.svg" alt="" className="h-6 w-6" />
      <span className="text-[13px] font-semibold tracking-tight text-fg">CapOpen</span>
      <span className="h-5 w-px bg-line" />
      <ProjectMenu />
      <ProjectName />
      <SaveStatus />
      <div className="flex-1" />
      <AiRunBar />
      <JobIndicator />
      <div className="flex items-center">
        <IconButton label={aiRun ? "Undo is available when the AI is done" : "Undo (Ctrl+Z)"} disabled={!snap?.canUndo || !!aiRun} onClick={undo}>
          <Undo2 size={16} />
        </IconButton>
        <IconButton label={aiRun ? "Redo is available when the AI is done" : "Redo (Ctrl+Shift+Z)"} disabled={!snap?.canRedo || !!aiRun} onClick={redo}>
          <Redo2 size={16} />
        </IconButton>
      </div>
      <span title={empty ? "Add a clip to the timeline to export" : "Export video (Ctrl+E)"}>
        <Button variant="primary" disabled={empty} onClick={openExport}>
          <Download size={15} /> Export
        </Button>
      </span>
    </header>
  );
}
