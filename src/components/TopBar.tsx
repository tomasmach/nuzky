import { useEffect, useRef, useState } from "react";
import { AlertCircle, Check, ChevronDown, Download, FilePlus2, FolderOpen, Loader2, Redo2, Undo2 } from "lucide-react";
import { api, errorText } from "../lib/api";
import { projectDuration, useEditor } from "../lib/store";
import { formatDuration } from "../lib/time";
import type { ProjectSummary } from "../lib/types";
import { Button, IconButton } from "./ui";

export const FORMATS = [
  { label: "9:16", hint: "Reels, TikTok, Shorts", width: 1080, height: 1920 },
  { label: "16:9", hint: "YouTube", width: 1920, height: 1080 },
  { label: "1:1", hint: "Square", width: 1080, height: 1080 },
  { label: "4:5", hint: "Instagram feed", width: 1080, height: 1350 },
];

export function formatLabel(width: number, height: number) {
  return FORMATS.find((f) => f.width === width && f.height === height)?.label ?? `${width}×${height}`;
}

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
      <span className="flex items-center gap-1 text-[12px] text-subtle" role="status">
        <Loader2 size={13} className="animate-spin" /> Saving…
      </span>
    );
  return (
    <span className="flex items-center gap-1 text-[12px] text-subtle" role="status">
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
      className="h-7 w-[260px] rounded border border-accent bg-raised px-1.5 text-[13px] text-fg outline-none"
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

  const run = async (fn: () => Promise<import("../lib/types").Snapshot>) => {
    setOpen(false);
    try {
      setSnap(await fn(), false);
      useEditor.setState({ timeUs: 0, thumbs: {}, waveforms: {} });
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
          <div className="px-2 pb-1 pt-1 text-[11px] font-semibold uppercase tracking-wide text-subtle">New project</div>
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
          <div className="border-t border-line px-2 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wide text-subtle">Recent</div>
          <div className="max-h-72 overflow-y-auto">
            {projects.length === 0 && <div className="px-2 py-2 text-[12px] text-subtle">No saved projects yet.</div>}
            {projects.map((p) => (
              <button
                key={p.path}
                type="button"
                disabled={p.path === current}
                onClick={() => run(() => api.openProject(p.path))}
                className="flex w-full items-center justify-between rounded-md px-2 py-1.5 text-left hover:bg-raised disabled:cursor-default disabled:bg-raised/60"
              >
                <span className="truncate text-[13px] text-fg">{p.name}</span>
                <span className="tabular shrink-0 pl-2 text-[11px] text-subtle">
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

function FormatSelect() {
  const canvas = useEditor((s) => s.snap?.project.canvas);
  const edit = useEditor((s) => s.edit);
  if (!canvas) return null;
  return (
    <label className="flex items-center gap-1.5 rounded-md border border-line bg-raised pl-2 text-[12px] text-muted">
      Format
      <span className="relative">
        <select
          aria-label="Canvas format"
          value={`${canvas.width}x${canvas.height}`}
          onChange={(e) => {
            const [w, h] = e.target.value.split("x").map(Number);
            edit({ type: "setCanvas", width: w, height: h });
          }}
          className="h-7 cursor-pointer appearance-none rounded-r-md bg-transparent pl-1 pr-6 text-[12px] font-medium text-fg outline-none"
        >
          {FORMATS.map((f) => (
            <option key={f.label} value={`${f.width}x${f.height}`}>
              {f.label} · {f.hint}
            </option>
          ))}
          {!FORMATS.some((f) => f.width === canvas.width && f.height === canvas.height) && (
            <option value={`${canvas.width}x${canvas.height}`}>{`${canvas.width}×${canvas.height}`}</option>
          )}
        </select>
        <ChevronDown size={13} className="pointer-events-none absolute right-1.5 top-2 text-muted" />
      </span>
    </label>
  );
}

function JobIndicator() {
  const jobs = useEditor((s) => s.jobs);
  const running = Object.values(jobs).filter((j) => j.status === "running" && j.kind !== "export");
  if (running.length === 0) return null;
  const j = running[0];
  return (
    <span className="flex items-center gap-1.5 text-[12px] text-muted" role="status">
      <Loader2 size={13} className="animate-spin text-accent" />
      <span className="max-w-[260px] truncate">{j.phase ?? j.label}</span>
      <span className="tabular text-subtle">{Math.round(j.progress * 100)}%</span>
      {running.length > 1 && <span className="text-subtle">+{running.length - 1}</span>}
    </span>
  );
}

export function TopBar() {
  const snap = useEditor((s) => s.snap);
  const { undo, redo } = useEditor.getState();
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
      <JobIndicator />
      <FormatSelect />
      <div className="flex items-center">
        <IconButton label="Undo (Ctrl+Z)" disabled={!snap?.canUndo} onClick={undo}>
          <Undo2 size={16} />
        </IconButton>
        <IconButton label="Redo (Ctrl+Shift+Z)" disabled={!snap?.canRedo} onClick={redo}>
          <Redo2 size={16} />
        </IconButton>
      </div>
      <span title={empty ? "Add a clip to the timeline to export" : "Export video (Ctrl+E)"}>
        <Button variant="primary" disabled={empty} onClick={() => useEditor.setState({ exportOpen: true })}>
          <Download size={15} /> Export
        </Button>
      </span>
    </header>
  );
}
