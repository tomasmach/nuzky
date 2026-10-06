import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { AlertCircle } from "lucide-react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorText } from "./lib/api";
import { receiveTranscript, speechJobEnded, useSpeech } from "./lib/speech";
import { deleteSelection, deleteSide, duplicateSelection, findClip, openExport, projectDuration, splitAtPlayhead, useEditor } from "./lib/store";
import { US } from "./lib/time";
import type { JobEvent, Snapshot, TimelineTranscript, Transport } from "./lib/types";
import { ExportDialog } from "./components/ExportDialog";
import { Inspector } from "./components/inspector/Inspector";
import { LeftPanel } from "./components/panel/LeftPanel";
import { MEDIA_EXTENSIONS, dropResolver, importPaths, pickAndImport } from "./components/panel/assets";
import { Preview } from "./components/preview/Preview";
import { Timeline } from "./components/timeline/Timeline";
import { Toasts } from "./components/Toasts";
import { TopBar } from "./components/TopBar";
import { Button } from "./components/ui";

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
        // Otherwise the focused button would also be clicked when Space is released.
        (document.activeElement as HTMLElement | null)?.blur();
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
  if ((job.kind === "captions" || job.kind === "transcript") && (job.status === "failed" || job.status === "cancelled")) speechJobEnded();
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
    offs.push(listen<{ jobId: string; transcript: TimelineTranscript }>("transcript-ready", (e) => receiveTranscript(e.payload.transcript)));
    offs.push(listen<string>("audio-ready", (e) => useEditor.getState().loadWaveform(e.payload, true)));
    offs.push(listen<Snapshot>("project-changed", (e) => useEditor.getState().setSnap(e.payload, true)));
    offs.push(listen<string>("engine-error", (e) => useEditor.setState({ engineError: e.payload })));
    offs.push(
      getCurrentWebview().onDragDropEvent((e) => {
        if (e.payload.type !== "drop" || useEditor.getState().snap?.recovery) return;
        const paths = e.payload.paths.filter((p) => MEDIA_EXTENSIONS.includes(p.split(".").pop()?.toLowerCase() ?? ""));
        if (paths.length === 0) {
          useEditor.getState().toast({ kind: "error", text: "Those files are not video, audio or images CapOpen can open." });
          return;
        }
        const dpr = window.devicePixelRatio || 1;
        const target = dropResolver?.(e.payload.position.x / dpr, e.payload.position.y / dpr) ?? undefined;
        importPaths(paths, target);
      }),
    );
    return () => offs.forEach((p) => p.then((off) => off()));
  }, []);
}

function useUiContext() {
  const selection = useEditor((s) => s.selection);
  const timeUs = useEditor((s) => s.timeUs);
  const epoch = useEditor((s) => s.snap?.sessionEpoch);
  useEffect(() => {
    if (!epoch) return;
    const timer = setTimeout(() => {
      void api.setUiContext(selection, timeUs).catch((e) => useEditor.getState().toast({ kind: "error", text: errorText(e) }));
    }, 200);
    return () => clearTimeout(timer);
  }, [selection, timeUs, epoch]);
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
      const snap = await api.resolveRecovery(action);
      useEditor.getState().setSnap(snap, true);
      useEditor.setState({ saveState: "saved" });
      if (action === "restore") useEditor.getState().toast({ kind: "success", text: "Previous version restored", action: { label: "Undo", run: () => void useEditor.getState().undo() } });
    } catch (e) {
      setError(errorText(e));
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

const clampTimeline = (h: number) => Math.round(Math.max(TIMELINE_MIN, Math.min(window.innerHeight - ABOVE_MIN, h)));

/** Drag handle between the preview row and the timeline; the height is remembered. */
function useTimelineHeight() {
  const [height, setHeight] = useState(() => clampTimeline(Number(localStorage.getItem(TIMELINE_KEY)) || TIMELINE_DEFAULT));
  const set = (h: number) => {
    const v = clampTimeline(h);
    setHeight(v);
    localStorage.setItem(TIMELINE_KEY, String(v));
  };
  useEffect(() => {
    const onResize = () => setHeight((h) => clampTimeline(h));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return [height, set] as const;
}

function Divider({ height, onChange }: { height: number; onChange: (h: number) => void }) {
  const [active, setActive] = useState(false);
  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const startY = e.clientY;
    setActive(true);
    const move = (ev: PointerEvent) => onChange(height - (ev.clientY - startY));
    const up = () => {
      setActive(false);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div
      role="separator"
      aria-orientation="horizontal"
      aria-label="Resize timeline"
      aria-valuenow={height}
      aria-valuemin={TIMELINE_MIN}
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

export default function App() {
  const snap = useEditor((s) => s.snap);
  const [timelineH, setTimelineH] = useTimelineHeight();
  useShortcuts();
  useBackendEvents();
  useUiContext();

  useEffect(() => {
    api.boot().then((boot) => {
      useEditor.setState({ previewUrl: boot.previewUrl, playing: boot.transport.playing, timeUs: boot.transport.tUs });
      useEditor.getState().setSnap(boot.snapshot);
      useEditor.setState({ saveState: "saved" });
    });
  }, []);

  // Drop selection entries that no longer exist after undo or delete.
  useEffect(() => {
    if (!snap) return;
    const sel = useEditor.getState().selection.filter((id) => findClip(snap.project, id));
    if (sel.length !== useEditor.getState().selection.length) useEditor.setState({ selection: sel });
  }, [snap]);

  if (!snap)
    return (
      <div className="flex h-full items-center justify-center text-[13px] text-muted" role="status">
        Starting CapOpen…
      </div>
    );

  return (
    <>
      <div className="flex h-full flex-col" inert={snap.recovery}>
        <TopBar />
        <div className="flex min-h-0 flex-1">
          <LeftPanel />
          <Preview />
          <Inspector />
        </div>
        <Divider height={timelineH} onChange={setTimelineH} />
        <Timeline height={timelineH} />
        <ExportDialog />
        <Toasts bottom={timelineH + 12} />
        <DragChip />
      </div>
      {snap.recovery && <RecoveryDialog key={snap.sessionEpoch} />}
    </>
  );
}
