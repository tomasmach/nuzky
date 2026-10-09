import { useState } from "react";
import { AlertCircle, Check, Download, FolderOpen, Loader2, Redo2, Sparkles, Undo2 } from "lucide-react";
import { useAgent } from "../lib/agent";
import { togglePanel, useDock } from "../lib/dock";
import { AI_EDITING, openExport, projectDuration, stopAiRun, useAiLocked, useEditor } from "../lib/store";
import { Button, IconButton, ProgressBar } from "./ui";
import { UpdateButton } from "./Updates";

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

/** Opens the launcher: recent projects, new ones and search (Ctrl+K). */
function ProjectsButton() {
  const open = useEditor((s) => s.launcherOpen);
  return (
    <Button variant="bar" pill aria-haspopup="dialog" aria-expanded={open} title="Projects (Ctrl+K)" onClick={() => useEditor.setState({ launcherOpen: !open })} className={open ? "bg-white/[.13]" : ""}>
      <FolderOpen size={15} /> Projects
    </Button>
  );
}

/** Background work. Export shows first and reopens its dialog; captions and transcripts open their tab. */
export function JobIndicator() {
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
      onClick={() => (j.kind === "export" ? useEditor.setState({ exportOpen: true }) : useEditor.setState({ view: "editor", panelTab: j.kind === "transcript" ? "transcript" : "captions" }))}
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
export function AiRunBar() {
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
        onClick={() => void stopAiRun().then(() => useEditor.setState({ view: "editor", launcherOpen: false }))}
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
      <ProjectsButton />
      <UpdateButton />
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
