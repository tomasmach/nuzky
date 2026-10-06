import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "./lib/api";
import { deleteSelection, duplicateSelection, findClip, openExport, projectDuration, splitAtPlayhead, useEditor } from "./lib/store";
import { US } from "./lib/time";
import type { JobEvent, Snapshot, Transport } from "./lib/types";
import { ExportDialog } from "./components/ExportDialog";
import { Inspector } from "./components/inspector/Inspector";
import { LeftPanel } from "./components/panel/LeftPanel";
import { MEDIA_EXTENSIONS, dropResolver, importPaths, pickAndImport } from "./components/panel/assets";
import { Preview } from "./components/preview/Preview";
import { Timeline } from "./components/timeline/Timeline";
import { Toasts } from "./components/Toasts";
import { TopBar } from "./components/TopBar";

const TEXT_INPUTS = new Set(["text", "search", "email", "number", "password", "url", "tel"]);

function isTyping(el: HTMLElement | null) {
  if (!el) return false;
  if (el.isContentEditable || el.tagName === "TEXTAREA" || el.tagName === "SELECT") return true;
  return el.tagName === "INPUT" && TEXT_INPUTS.has((el as HTMLInputElement).type);
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
      if (isTyping(target) || useEditor.getState().exportOpen) return;
      const s = useEditor.getState();
      const mod = e.ctrlKey || e.metaKey;
      const fps = s.snap?.project.canvas.fps ?? 30;
      const key = e.key.toLowerCase();
      // A focused slider keeps its own arrow, Home and End keys.
      const onRange = target instanceof HTMLInputElement && target.type === "range";
      if (key === " ") {
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
    offs.push(listen<string>("audio-ready", (e) => useEditor.getState().loadWaveform(e.payload, true)));
    offs.push(listen<Snapshot>("project-changed", (e) => useEditor.getState().setSnap(e.payload)));
    offs.push(listen<string>("engine-error", (e) => useEditor.setState({ engineError: e.payload })));
    offs.push(
      getCurrentWebview().onDragDropEvent((e) => {
        if (e.payload.type !== "drop") return;
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
if (import.meta.env.DEV) Object.assign(window, { __capopen: { importPaths, store: useEditor, api } });

export default function App() {
  const snap = useEditor((s) => s.snap);
  const [timelineH, setTimelineH] = useTimelineHeight();
  useShortcuts();
  useBackendEvents();

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
    <div className="flex h-full flex-col">
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
  );
}
