import { useState } from "react";
import { AlertCircle, Check, Download, FolderOpen, History, Loader2, Redo2, Sparkles, Undo2 } from "lucide-react";
import { useAgent } from "../lib/agent";
import { api, errorText } from "../lib/api";
import { togglePanel, useDock } from "../lib/dock";
import { openCover, useCover } from "../lib/cover";
import { useStyle } from "../lib/style";
import { when } from "../lib/time";
import { AI_EDITING, openExport, projectDuration, restoreVersion, stopAiRun, useAiLocked, useEditor, whenIdle } from "../lib/store";
import type { ProjectVersion } from "../lib/types";
import { Button, IconButton, Menu, ProgressBar } from "./ui";
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

/** Background work. Export shows first and reopens its dialog; captions and transcripts open their tab; a preview proxy can be stopped. */
export function JobIndicator() {
  const jobs = useEditor((s) => s.jobs);
  const running = Object.values(jobs).filter((j) => j.status === "running");
  if (running.length === 0) return null;
  const j =
    running.find((x) => x.kind === "export") ??
    running.find((x) => x.kind === "captions" || x.kind === "transcript") ??
    // Ahead of the audio prepared beside it on import, so its Stop is there.
    running.find((x) => x.kind === "proxy") ??
    running[0];
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
  if (j.kind === "proxy")
    return (
      <span className="bar flex h-8 items-center gap-1.5 rounded-full pl-3 pr-1 text-[12px] font-medium text-fg" role="status">
        {body}
        <Button pill className="h-6 shrink-0 px-2.5 text-[12px]" title="Stop preparing the preview. It keeps playing the original file, which can stutter." onClick={() => void api.cancelJob(j.id)}>
          Stop
        </Button>
      </span>
    );
  // A cover export reopens the Export dialog; picking frames, masks and model downloads open the cover editor.
  const coverExport = j.id.startsWith("cover-export:");
  const cover = (j.kind === "cover" && !coverExport) || j.kind === "vision-models";
  return (
    <button
      type="button"
      title={j.kind === "export" || coverExport ? "Show export progress" : cover ? "Show the cover" : j.kind === "transcript" ? "Show transcript" : j.kind === "style" ? "Show learning" : "Show captions"}
      onClick={() =>
        j.kind === "export" || coverExport
          ? useEditor.setState({ view: "editor", exportOpen: true })
          : cover
            ? (useEditor.setState({ view: "editor" }), useCover.getState().open || openCover())
            : j.kind === "style"
              ? useStyle.setState({ learnOpen: true, learnTab: useStyle.getState().projectsLearning?.jobId === j.id ? "projects" : "videos" })
              : useEditor.setState({ view: "editor", panelTab: j.kind === "transcript" ? "transcript" : "captions" })
      }
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

/** Versions listed in the menu, so it fits the smallest window. */
const VERSIONS_SHOWN = 20;

/** The kept versions of the project, newest first; choosing one restores it as one undo step. */
function VersionsButton() {
  const aiRun = useEditor((s) => s.aiRun);
  const [menu, setMenu] = useState<{ versions: ProjectVersion[]; at: { x: number; y: number; align: "end" }; keyboard: boolean; back: HTMLElement } | null>(null);
  return (
    <>
      <IconButton
        round
        aria-haspopup="menu"
        aria-expanded={!!menu}
        label={aiRun ? "Versions are available when the AI is done" : "Versions"}
        disabled={!!aiRun}
        onClick={async (e) => {
          const back = e.currentTarget;
          const keyboard = e.detail === 0;
          try {
            // The list then holds the edits made just before.
            await whenIdle();
            const versions = await api.listVersions(VERSIONS_SHOWN);
            const r = back.getBoundingClientRect();
            setMenu({ versions, at: { x: r.right, y: r.bottom + 6, align: "end" }, keyboard, back });
          } catch (error) {
            useEditor.getState().toast({ kind: "error", text: errorText(error) });
          }
        }}
      >
        <History size={16} />
      </IconButton>
      {menu && (
        <Menu
          label="Versions"
          minWidth={280}
          at={menu.at}
          keyboard={menu.keyboard}
          items={menu.versions.map((version) => ({
            label: version.label,
            icon: version.ai ? <Sparkles size={14} /> : undefined,
            shortcut: when(version.atMs),
            checked: version.current,
            run: version.current ? undefined : () => void restoreVersion(version),
          }))}
          onClose={(chose) => {
            const back = menu.back;
            setMenu(null);
            if (!chose) back.focus();
          }}
        />
      )}
    </>
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
        <VersionsButton />
      </div>
      <Button variant="primary" pill disabled={empty} disabledReason="Add a clip to the timeline to export" title="Export video (Ctrl+E)" onClick={openExport}>
        <Download size={15} /> Export
      </Button>
    </header>
  );
}
