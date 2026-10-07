import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { AlertCircle } from "lucide-react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorText, plainError } from "./lib/api";
import { followPointer } from "./lib/drag";
import { setLimits } from "./lib/limits";
import { useSpeech } from "./lib/speech";
import { currentEpoch, deleteSelection, deleteSide, duplicateSelection, openExport, projectDuration, splitAtPlayhead, undoAction, useEditor } from "./lib/store";
import { US, formatDuration } from "./lib/time";
import type { JobEvent, ProjectSummary, Snapshot, Transport } from "./lib/types";
import { ConnectAgentDialog } from "./components/ConnectAgentDialog";
import { ExportDialog } from "./components/ExportDialog";
import { Inspector } from "./components/inspector/Inspector";
import { LeftPanel } from "./components/panel/LeftPanel";
import { MEDIA_EXTENSIONS, dropResolver, importPaths, pickAndImport } from "./components/panel/assets";
import { Preview } from "./components/preview/Preview";
import { Timeline } from "./components/timeline/Timeline";
import { Toasts } from "./components/Toasts";
import { TopBar } from "./components/TopBar";
import { Button, DisabledHint } from "./components/ui";

const isMedia = (path: string) => MEDIA_EXTENSIONS.includes(path.split(".").pop()?.toLowerCase() ?? "");

const TEXT_INPUTS = new Set(["text", "search", "email", "number", "password", "url", "tel"]);

function isTyping(el: HTMLElement | null) {
  if (!el) return false;
  if (el.isContentEditable || el.tagName === "TEXTAREA" || el.tagName === "SELECT") return true;
  return el.tagName === "INPUT" && TEXT_INPUTS.has((el as HTMLInputElement).type);
}

/** Controls whose own Space action (toggle, pick a menu item) wins over play and pause. */
function usesSpace(el: HTMLElement | null) {
  if (!(el instanceof Element)) return false;
  if (el instanceof HTMLInputElement && (el.type === "checkbox" || el.type === "radio")) return true;
  return /^(checkbox|radio|switch|menuitem|menuitemradio|menuitemcheckbox|option)$/.test(el.getAttribute("role") ?? "");
}

/** Swallows the next Space release, which would otherwise click the focused button. */
function suppressSpaceRelease() {
  const up = (e: KeyboardEvent) => {
    if (e.key !== " ") return;
    e.preventDefault();
    window.removeEventListener("keyup", up, true);
  };
  window.addEventListener("keyup", up, true);
}

function DragChip() {
  const drag = useEditor((s) => s.assetDrag);
  const asset = useEditor((s) => s.snap?.project.assets.find((a) => a.id === s.assetDrag?.assetId));
  if (!drag || !asset) return null;
  return (
    <div className="pointer-events-none fixed z-[120] rounded-md bg-accent px-2 py-1 text-[12px] font-medium text-black shadow-lg" style={{ left: drag.x + 12, top: drag.y + 10 }}>
      {asset.name}
    </div>
  );
}

function useShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (isTyping(target) || useEditor.getState().exportOpen || useEditor.getState().snap?.recovery) return;
      const s = useEditor.getState();
      const mod = e.ctrlKey || e.metaKey;
      const fps = s.snap?.project.canvas.fps ?? 30;
      const key = e.key.toLowerCase();
      // A focused slider keeps its own arrow, Home and End keys.
      const onRange = target instanceof HTMLInputElement && target.type === "range";
      if (key === " ") {
        if (usesSpace(target)) return;
        e.preventDefault();
        // A focused button would also be clicked when Space is released; that release is held back
        // instead of taking focus away, so the keyboard user stays where they were.
        // Holding Space plays or pauses once.
        if (e.repeat) return;
        suppressSpaceRelease();
        s.togglePlay();
      } else if (mod && key === "z" && !e.shiftKey) {
        e.preventDefault();
        s.undo();
      } else if (mod && (key === "y" || (key === "z" && e.shiftKey))) {
        e.preventDefault();
        s.redo();
      } else if (mod && key === "i") {
        e.preventDefault();
        pickAndImport();
      } else if (mod && key === "e") {
        e.preventDefault();
        openExport();
      } else if (mod && key === "d") {
        e.preventDefault();
        duplicateSelection();
      } else if (!mod && key === "s") {
        e.preventDefault();
        splitAtPlayhead();
      } else if (!mod && (key === "q" || key === "w")) {
        e.preventDefault();
        deleteSide(key === "q" ? "left" : "right");
      } else if (key === "delete" || key === "backspace") {
        e.preventDefault();
        deleteSelection();
      } else if ((key === "arrowleft" || key === "arrowright") && !onRange) {
        e.preventDefault();
        const dir = key === "arrowleft" ? -1 : 1;
        s.seek(s.timeUs + dir * (e.shiftKey ? US : US / fps));
      } else if (key === "home" && !onRange) {
        s.seek(0);
      } else if (key === "end" && !onRange) {
        if (s.snap) s.seek(projectDuration(s.snap.project));
      } else if (key === "escape") {
        s.select([]);
      } else if (key === "+" || key === "=") {
        s.setZoom(s.zoom * 1.3);
      } else if (key === "-") {
        s.setZoom(s.zoom / 1.3);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

function onJob(job: JobEvent) {
  const { toast, exportOpen, exportJobId } = useEditor.getState();
  useEditor.setState({ jobs: { ...useEditor.getState().jobs, [job.id]: job } });
  if (job.kind === "captions" && job.status === "done") toast({ kind: "success", text: `Added ${job.output ?? "captions"}` });
  if (job.kind === "captions" && job.status === "failed") toast({ kind: "error", text: `Captions failed: ${job.message}` });
  if (job.kind === "transcript" && job.status === "done") toast({ kind: "success", text: `Transcript ready: ${job.output ?? "words"}` });
  if (job.kind === "transcript" && job.status === "failed") toast({ kind: "error", text: `Transcript failed: ${job.message}` });
  if (job.kind === "audio" && job.status === "failed") toast({ kind: "error", text: `${job.label} failed: ${job.message}` });
  // The dialog shows the result itself; with it closed, a toast reports it.
  if (job.kind === "export" && job.id === exportJobId && !exportOpen) {
    if (job.status === "done") {
      toast({ kind: "success", text: "Export finished", action: job.output ? { label: "Show in folder", run: () => revealItemInDir(job.output!) } : undefined });
      useEditor.setState({ exportJobId: null });
    }
    if (job.status === "failed") toast({ kind: "error", text: `Export failed: ${job.message}`, action: { label: "Details", run: () => useEditor.setState({ exportOpen: true }) } });
  }
}

function useBackendEvents() {
  useEffect(() => {
    const offs: Promise<() => void>[] = [];
    offs.push(listen<Transport>("transport", (e) => useEditor.setState({ playing: e.payload.playing, timeUs: e.payload.tUs })));
    offs.push(
      listen<{ revision: number; error: string | null }>("saved", (e) => {
        const snap = useEditor.getState().snap;
        if (e.payload.error) useEditor.setState({ saveState: "error" });
        else if (!snap || e.payload.revision >= snap.revision) useEditor.setState({ saveState: "saved" });
      }),
    );
    offs.push(listen<JobEvent>("job", (e) => onJob(e.payload)));
    offs.push(listen("transcripts-changed", () => useSpeech.setState((s) => ({ stored: s.stored + 1 }))));
    offs.push(listen<string>("audio-ready", (e) => useEditor.getState().loadWaveform(e.payload, true)));
    offs.push(listen<Snapshot>("project-changed", (e) => useEditor.getState().setSnap(e.payload, true)));
    // null: the preview recovered, e.g. after a failed frame.
    offs.push(listen<string | null>("engine-error", (e) => useEditor.setState({ engineError: e.payload })));
    let hasMedia = false;
    // The run seen last, also one already open at boot; a snapshot can clear `aiRun` before this event.
    let lastRun: string | null = useEditor.getState().aiRun;
    // The revision the run started from, unknown for a run already open at boot.
    let runStart: number | null = null;
    const unwatch = useEditor.subscribe((s, prev) => {
      if (s.aiRun) lastRun = s.aiRun;
      if (s.aiRun && !prev.aiRun) runStart = prev.snap ? (s.snap?.revision ?? null) : null;
    });
    offs.push(
      listen<string | null>("run-changed", (e) => {
        const { toast, snap } = useEditor.getState();
        const aiRun = lastRun;
        lastRun = e.payload;
        useEditor.setState({ aiRun: e.payload });
        // The run's last change arrived before this event, so Undo here removes the whole run; a run
        // that changed nothing offers none, as it would undo the step before it.
        const changed = !!snap && snap.revision !== runStart;
        if (aiRun && !e.payload) toast({ kind: "success", text: `AI edit done: ${aiRun}`, action: snap && changed ? undoAction(snap) : undefined });
      }),
    );
    offs.push(
      listen<string>("close-save-failed", (e) =>
        useEditor.getState().toast({ kind: "error", text: `Your last changes could not be saved: ${e.payload}. Free some disk space and close again, or close again to quit without them.` }),
      ),
    );
    offs.push(
      getCurrentWebview().onDragDropEvent((e) => {
        const dpr = window.devicePixelRatio || 1;
        const p = e.payload;
        // While media files are dragged over the window, the timeline shows where they would land.
        if (p.type === "enter") hasMedia = p.paths.some(isMedia);
        if ((p.type === "enter" || p.type === "over") && hasMedia) useEditor.setState({ fileDrag: { x: p.position.x / dpr, y: p.position.y / dpr } });
        if (p.type === "leave" || p.type === "drop") useEditor.setState({ fileDrag: null });
        if (p.type !== "drop" || useEditor.getState().snap?.recovery) return;
        const paths = p.paths.filter(isMedia);
        if (paths.length === 0) {
          useEditor.getState().toast({ kind: "error", text: "Those files are not video, audio or images CapOpen can open." });
          return;
        }
        const target = dropResolver?.(p.position.x / dpr, p.position.y / dpr) ?? undefined;
        importPaths(paths, target);
      }),
    );
    return () => {
      unwatch();
      offs.forEach((p) => p.then((off) => off()));
    };
  }, []);
}

const UI_CONTEXT_MS = 250;

/**
 * Tells agents what is selected and where the playhead is: at most every 250 ms, ending on the
 * latest values, also while playing. It follows the store directly, so the playhead moving never
 * re-renders the app.
 */
function useUiContext() {
  useEffect(() => {
    let timer = 0;
    let lastSent = 0;
    const send = () => {
      timer = 0;
      lastSent = Date.now();
      const { selection, timeUs, snap } = useEditor.getState();
      if (!snap) return;
      void api.setUiContext(selection, timeUs).catch((e) => useEditor.getState().toast({ kind: "error", text: errorText(e) }));
    };
    const unsubscribe = useEditor.subscribe((s, prev) => {
      if (timer || !s.snap?.sessionEpoch) return;
      if (s.selection === prev.selection && s.timeUs === prev.timeUs && s.snap.sessionEpoch === prev.snap?.sessionEpoch) return;
      timer = window.setTimeout(send, Math.max(0, UI_CONTEXT_MS - (Date.now() - lastSent)));
    });
    return () => {
      unsubscribe();
      window.clearTimeout(timer);
    };
  }, []);
}

function RecoveryDialog() {
  const dialog = useRef<HTMLDialogElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    dialog.current?.showModal();
    dialog.current?.querySelector<HTMLButtonElement>("[data-autofocus]")?.focus();
  }, []);
  const resolve = async (action: "keep" | "restore") => {
    setBusy(true);
    setError(null);
    try {
      const snap = await api.resolveRecovery(action, currentEpoch());
      useEditor.getState().setSnap(snap, true);
      useEditor.setState({ saveState: "saved" });
      if (action === "restore") useEditor.getState().toast({ kind: "success", text: "Previous version restored", action: undoAction(snap) });
    } catch (e) {
      setError(plainError(errorText(e)));
      setBusy(false);
    }
  };
  return (
    <dialog ref={dialog} onCancel={(e) => e.preventDefault()} aria-labelledby="recovery-title" aria-describedby="recovery-description" className="fixed inset-0 m-auto w-[440px] rounded-lg border border-line bg-panel p-6 text-fg shadow-2xl backdrop:bg-black/60">
      <h2 id="recovery-title" className="text-[16px] font-semibold">An AI edit didn't finish</h2>
      <p id="recovery-description" className="mt-3 text-[13px] text-muted">Keep the changes it made, or go back to the version before it started?</p>
      {error && <p role="alert" className="mt-3 flex items-start gap-2 text-[13px] text-danger"><AlertCircle size={16} className="shrink-0" />{error}</p>}
      <div className="mt-6 flex justify-end gap-2">
        <Button disabled={busy} onClick={() => void resolve("restore")}>Restore previous version</Button>
        <Button variant="primary" data-autofocus disabled={busy} onClick={() => void resolve("keep")}>Keep changes</Button>
      </div>
    </dialog>
  );
}

const TIMELINE_KEY = "capopen.timelineHeight";
const TIMELINE_DEFAULT = 300;
const TIMELINE_MIN = 160;
/** Space kept for the top bar and a usable preview above the timeline. */
const ABOVE_MIN = 48 + 300;

const timelineMax = () => Math.max(TIMELINE_MIN, window.innerHeight - ABOVE_MIN);
const clampTimeline = (h: number) => Math.round(Math.min(timelineMax(), Math.max(TIMELINE_MIN, h)));

/** Drag handle between the preview row and the timeline; the height is remembered. */
function useTimelineHeight() {
  const [height, setHeight] = useState(() => clampTimeline(Number(localStorage.getItem(TIMELINE_KEY)) || TIMELINE_DEFAULT));
  const [max, setMax] = useState(timelineMax);
  const set = (h: number) => {
    const v = clampTimeline(h);
    setHeight(v);
    localStorage.setItem(TIMELINE_KEY, String(v));
  };
  useEffect(() => {
    const onResize = () => {
      setMax(timelineMax());
      setHeight((h) => clampTimeline(h));
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return [height, set, max] as const;
}

function Divider({ height, max, onChange }: { height: number; max: number; onChange: (h: number) => void }) {
  const [active, setActive] = useState(false);
  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const startY = e.clientY;
    setActive(true);
    // A cancelled drag keeps the height reached so far.
    followPointer({ move: (ev) => onChange(height - (ev.clientY - startY)), up: () => setActive(false), cancel: () => setActive(false) });
  };
  return (
    <div
      role="separator"
      aria-orientation="horizontal"
      aria-label="Resize timeline"
      aria-valuenow={height}
      aria-valuemin={TIMELINE_MIN}
      aria-valuemax={max}
      tabIndex={0}
      title="Drag to resize the timeline. Double-click to reset."
      onPointerDown={onPointerDown}
      onDoubleClick={() => onChange(TIMELINE_DEFAULT)}
      onKeyDown={(e) => {
        if (e.key === "ArrowUp" || e.key === "ArrowDown") {
          e.preventDefault();
          e.stopPropagation();
          onChange(height + (e.key === "ArrowUp" ? 24 : -24));
        }
      }}
      className="group relative z-30 -my-[3px] h-[7px] shrink-0 cursor-row-resize"
    >
      <div className={`absolute inset-x-0 top-[2px] h-[3px] transition-colors duration-[120ms] ${active ? "bg-accent" : "group-hover:bg-accent/60"}`} />
    </div>
  );
}

// Test hook for WebDriver runs; native file dialogs cannot be automated.
if (import.meta.env.DEV) Object.assign(window, { __capopen: { importPaths, store: useEditor, speech: useSpeech, api } });

async function startEditor() {
  const boot = await api.boot();
  setLimits(boot.limits);
  useEditor.setState({ previewUrl: boot.previewUrl, playing: boot.transport.playing, timeUs: boot.transport.tUs });
  if (boot.engineError) useEditor.setState({ engineError: boot.engineError });
  useEditor.getState().setSnap(boot.snapshot, true, true);
  useEditor.setState({ saveState: "saved" });
  // A warning stays until dismissed, so the reason a different project opened is not missed.
  // Once per launch: StrictMode and Try again can start the editor more than once.
  if (boot.startupNotice && !startupNoticeShown) {
    startupNoticeShown = true;
    useEditor.getState().toast({ kind: "warning", text: boot.startupNotice });
  }
}

let startupNoticeShown = false;

/** Until the editor has a project: "Starting", or why starting failed with ways to go on. */
function BootScreen() {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(true);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  /** `open` first opens or creates a project, for when the last one is what failed. */
  const start = async (open?: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await open?.();
      await startEditor();
    } catch (e) {
      setError(plainError(errorText(e)));
      setBusy(false);
      api.listProjects().then(setProjects, () => setProjects([]));
    }
  };
  useEffect(() => void start(), []);

  if (error === null)
    return (
      <div className="flex h-full items-center justify-center text-[13px] text-muted" role="status">
        Starting CapOpen…
      </div>
    );
  return (
    <div className="flex h-full items-center justify-center p-6">
      <div className="flex w-[440px] flex-col gap-4">
        <div role="alert" className="flex flex-col gap-2">
          <h1 className="text-[16px] font-semibold text-fg">CapOpen could not start</h1>
          <p className="flex items-start gap-2 text-[13px] text-danger">
            <AlertCircle size={16} className="mt-px shrink-0" />
            {error}
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="primary" disabled={busy} onClick={() => void start()}>
            Try again
          </Button>
          <Button disabled={busy} onClick={() => void start(() => api.newProject(1080, 1920))}>
            New project
          </Button>
        </div>
        {projects.length > 0 && (
          <div className="flex flex-col gap-1">
            <h2 className="text-[11px] font-semibold uppercase tracking-wide text-muted">Open another project</h2>
            <div className="max-h-64 overflow-y-auto">
              {projects.map((p) => (
                <button
                  key={p.path}
                  type="button"
                  disabled={busy}
                  onClick={() => void start(() => api.openProject(p.path))}
                  className="flex w-full items-center justify-between rounded-md px-2 py-1.5 text-left hover:bg-raised disabled:cursor-not-allowed disabled:opacity-40"
                >
                  <span className="truncate text-[13px] text-fg">{p.name}</span>
                  <span className="tabular shrink-0 pl-2 text-[11px] text-muted">{formatDuration(p.durationUs)}</span>
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export default function App() {
  const snap = useEditor((s) => s.snap);
  const [timelineH, setTimelineH, timelineMaxH] = useTimelineHeight();
  useShortcuts();
  useBackendEvents();
  useUiContext();

  if (!snap) return <BootScreen />;

  return (
    <>
      <div className="flex h-full flex-col" inert={snap.recovery}>
        <TopBar />
        <div className="flex min-h-0 flex-1">
          <LeftPanel />
          <Preview />
          <Inspector />
        </div>
        <Divider height={timelineH} max={timelineMaxH} onChange={setTimelineH} />
        <Timeline height={timelineH} />
        <ExportDialog />
        <ConnectAgentDialog />
        <Toasts bottom={timelineH + 12} />
        <DisabledHint />
        <DragChip />
      </div>
      {snap.recovery && <RecoveryDialog key={snap.sessionEpoch} />}
    </>
  );
}
