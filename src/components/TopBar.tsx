import { useEffect, useRef, useState } from "react";
import { AlertCircle, Check, ChevronDown, Download, FilePlus2, FolderOpen, Loader2, Redo2, Sparkles, Undo2 } from "lucide-react";
import { api, errorText } from "../lib/api";
import { useAgent } from "../lib/agent";
import { togglePanel, useDock } from "../lib/dock";
import { FORMATS } from "../lib/presets";
import { AI_EDITING, openExport, projectDuration, stopAiRun, switchProject, useAiLocked, useEditor } from "../lib/store";
import { formatDuration } from "../lib/time";
import type { ProjectSummary, Snapshot } from "../lib/types";
import { Button, IconButton, ProgressBar } from "./ui";

function SaveStatus() {
  const saveState = useEditor((s) => s.saveState);
  if (saveState === "error")
    return (
      <span className="flex shrink-0 items-center gap-1 text-[12px] text-danger" role="status">
        <AlertCircle size={13} /> Not saved, retrying on next edit
      </span>
    );
  if (saveState === "saving")
    return (
      <span className="flex shrink-0 items-center gap-1 text-[12px] text-muted" role="status">
        <Loader2 size={12} className="animate-spin" /> Saving…
      </span>
    );
  return (
    <span className="flex shrink-0 items-center gap-1 text-[12px] text-muted" role="status">
      <Check size={12} /> Saved
    </span>
  );
}

function ProjectName() {
  const name = useEditor((s) => s.snap?.project.name ?? "");
  const edit = useEditor((s) => s.edit);
  const locked = useAiLocked();
  const [draft, setDraft] = useState<string | null>(null);

  if (draft === null)
    return (
      <button
        type="button"
        title={locked ? AI_EDITING : "Rename project"}
        aria-disabled={locked || undefined}
        onClick={locked ? undefined : () => setDraft(name)}
        className="max-w-[260px] truncate rounded-md px-1.5 py-0.5 text-[13px] font-semibold text-fg hover:bg-white/[.08] aria-disabled:cursor-not-allowed aria-disabled:hover:bg-transparent"
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
      autoFocus
      aria-label="Project name"
      value={draft}
      onFocus={(e) => e.currentTarget.select()}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") setDraft(null);
      }}
      className="h-7 w-[260px] rounded-md border border-accent bg-white/[.06] px-1.5 text-center text-[13px] font-semibold text-fg"
    />
  );
}

function ProjectMenu() {
  const [open, setOpen] = useState(false);
  // null until the list has loaded, so a failed load never claims there are no projects.
  const [projects, setProjects] = useState<ProjectSummary[] | null>(null);
  const current = useEditor((s) => s.snap?.path);
  const { toast } = useEditor.getState();
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    api.listProjects().then(setProjects, (e) => {
      setProjects(null);
      toast({ kind: "error", text: errorText(e) });
    });
    const close = (e: PointerEvent) => !ref.current?.contains(e.target as Node) && setOpen(false);
    // Esc closes the menu only, before the editor's shortcuts would also clear the selection.
    const esc = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      setOpen(false);
      ref.current?.querySelector("button")?.focus();
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", esc, true);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", esc, true);
    };
  }, [open]);

  const run = async (fn: () => Promise<Snapshot>) => {
    setOpen(false);
    try {
      await switchProject(fn);
      // Opening another project starts a new session, and setSnap already drops (and frees) its media previews.
      useEditor.setState({ timeUs: 0 });
    } catch (e) {
      toast({ kind: "error", text: errorText(e) });
    }
  };

  return (
    <div className="relative" ref={ref}>
      <Button variant="bar" pill aria-expanded={open} aria-haspopup="menu" onClick={() => setOpen(!open)} className={open ? "bg-white/[.13]" : ""}>
        <FolderOpen size={15} /> Projects <ChevronDown size={13} className="text-muted" />
      </Button>
      {open && (
        <div className="overlay absolute left-0 top-10 z-50 w-80 rounded-xl p-1.5">
          <div className="px-2 pb-1 pt-1.5 text-[11px] font-semibold text-muted">New project</div>
          <div className="grid grid-cols-4 gap-1 pb-2">
            {FORMATS.map((f) => (
              <button
                key={f.label}
                type="button"
                title={f.hint}
                onClick={() => run(() => api.newProject(f.width, f.height))}
                className="flex flex-col items-center gap-1 rounded-lg px-1 py-2 text-[12px] text-muted hover:bg-white/[.08] hover:text-fg"
              >
                <FilePlus2 size={16} />
                {f.label}
              </button>
            ))}
          </div>
          <div className="border-t border-white/[.08] px-2 pb-1 pt-2 text-[11px] font-semibold text-muted">Recent</div>
          <div className="max-h-72 overflow-y-auto">
            {projects?.length === 0 && <div className="px-2 py-2 text-[12px] text-muted">No saved projects yet.</div>}
            {projects?.map((p) => (
              <button
                key={p.path}
                type="button"
                disabled={p.path === current}
                onClick={() => run(() => api.openProject(p.path))}
                className="flex w-full items-center justify-between rounded-md px-2 py-1.5 text-left hover:bg-white/[.08] disabled:cursor-default disabled:bg-white/[.06]"
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
      <Loader2 size={14} className="shrink-0 animate-spin text-accent" />
      <span className="max-w-[220px] truncate">{j.kind === "export" ? "Exporting" : (j.phase ?? j.label)}</span>
      {pct && <span className="tabular text-muted">{pct}</span>}
      {j.kind === "export" && <ProgressBar value={j.progress} label={j.label} className="w-16" />}
      {running.length > 1 && <span className="text-muted">+{running.length - 1}</span>}
    </>
  );
  if (j.kind === "audio")
    return (
      <span className="bar flex h-8 items-center gap-1.5 rounded-full px-3 text-[12px] font-medium text-fg" role="status">
        {body}
      </span>
    );
  return (
    <button
      type="button"
      title={j.kind === "export" ? "Show export progress" : j.kind === "transcript" ? "Show transcript" : "Show captions"}
      onClick={() => (j.kind === "export" ? useEditor.setState({ exportOpen: true }) : useEditor.setState({ panelTab: j.kind === "transcript" ? "transcript" : "captions" }))}
      className="bar flex h-8 items-center rounded-full px-3 text-[12px] font-medium text-fg transition-colors duration-[120ms] hover:bg-white/[.13]"
    >
      <span className="flex items-center gap-1.5" role="status">
        {body}
      </span>
    </button>
  );
}

/** Opens and closes the AI panel; it stays open or closed across launches. */
function AiToggle() {
  const open = useDock((s) => s.open);
  const working = useAgent((s) => s.status !== "idle");
  return (
    <Button
      variant="bar"
      pill
      aria-pressed={open}
      title={open ? "Close AI (Ctrl+J)" : working ? "Open AI (Ctrl+J). It is still working." : "Open AI (Ctrl+J)"}
      onClick={togglePanel}
      className={open ? "bg-white/[.15]" : ""}
    >
      {working && !open ? <Loader2 size={15} className="animate-spin text-accent" /> : <Sparkles size={15} />} AI
    </Button>
  );
}

/** While an agent edits, what it is doing and a way to take over. */
function AiRunBar() {
  const label = useEditor((s) => s.aiRun);
  if (!label) return null;
  return (
    <div className="bar flex h-8 min-w-0 max-w-[420px] items-center gap-2 rounded-full pl-3 pr-1 text-[12px] font-medium text-fg" role="status">
      <Sparkles size={14} className="shrink-0 text-accent" />
      <span className="truncate">
        AI is editing <span className="font-normal text-muted">· {label}</span>
      </span>
      <Button
        pill
        className="h-6 shrink-0 px-2.5 text-[12px]"
        title="Stop the AI, keep what it did so far, and edit yourself"
        // The backend stops the AI panel's agent too, so it does not go on editing.
        onClick={() => void stopAiRun()}
      >
        Stop and edit
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
    <header className="flex h-12 shrink-0 items-center gap-2 px-1.5">
      <ProjectMenu />
      {/* The name sits in the middle of the space the two sides leave, so a long AI label never covers it.
          The save state hangs off its right edge, so "Saving…" and "Saved" never move the name. */}
      <div className="flex min-w-0 flex-1 justify-center">
        <div className="relative flex min-w-0 items-center">
          <ProjectName />
          <div className="absolute left-full ml-2 whitespace-nowrap">
            <SaveStatus />
          </div>
        </div>
      </div>
      <AiRunBar />
      <JobIndicator />
      <AiToggle />
      <div className="bar flex items-center rounded-full">
        <IconButton round label={aiRun ? "Undo is available when the AI is done" : "Undo (Ctrl+Z)"} disabled={!snap?.canUndo || !!aiRun} onClick={undo}>
          <Undo2 size={16} />
        </IconButton>
        <IconButton round label={aiRun ? "Redo is available when the AI is done" : "Redo (Ctrl+Shift+Z)"} disabled={!snap?.canRedo || !!aiRun} onClick={redo}>
          <Redo2 size={16} />
        </IconButton>
      </div>
      <Button variant="primary" pill disabled={empty} disabledReason="Add a clip to the timeline to export" title="Export video (Ctrl+E)" onClick={openExport}>
        <Download size={15} /> Export
      </Button>
    </header>
  );
}
