import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import { api } from "./lib/api";
import { allClips, findClip, MAIN_TRACK, projectDuration, useEditor } from "./lib/store";
import { US } from "./lib/time";
import type { JobEvent, Snapshot, Transport } from "./lib/types";
import { ExportDialog } from "./components/ExportDialog";
import { Inspector } from "./components/Inspector";
import { MediaPanel, dropResolver, importPaths, pickAndImport, MEDIA_EXTENSIONS } from "./components/MediaPanel";
import { Preview } from "./components/Preview";
import { Timeline } from "./components/Timeline";
import { TopBar } from "./components/TopBar";

function Toasts() {
  const toasts = useEditor((s) => s.toasts);
  const dismiss = useEditor((s) => s.dismissToast);
  return (
    <div className="pointer-events-none fixed left-1/2 top-14 z-[90] flex -translate-x-1/2 flex-col items-center gap-2" aria-live="polite">
      {toasts.map((t) => (
        <div
          key={t.id}
          role={t.kind === "error" ? "alert" : "status"}
          className={`pointer-events-auto flex max-w-[560px] items-center gap-2 rounded-lg border px-3 py-2 text-[13px] shadow-xl shadow-black/50 ${
            t.kind === "error" ? "border-danger/50 bg-[#2a1416] text-fg" : "border-line bg-raised text-fg"
          }`}
        >
          {t.kind === "error" ? (
            <AlertCircle size={16} className="shrink-0 text-danger" />
          ) : t.kind === "success" ? (
            <CheckCircle2 size={16} className="shrink-0 text-accent" />
          ) : (
            <Info size={16} className="shrink-0 text-muted" />
          )}
          <span className="min-w-0 break-words">{t.text}</span>
          {t.action && (
            <button
              type="button"
              className="shrink-0 rounded px-1.5 py-0.5 font-medium text-accent hover:bg-accent/10"
              onClick={() => {
                t.action!.run();
                dismiss(t.id);
              }}
            >
              {t.action.label}
            </button>
          )}
          <button type="button" aria-label="Dismiss" className="shrink-0 text-subtle hover:text-fg" onClick={() => dismiss(t.id)}>
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
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

function isTyping(e: KeyboardEvent) {
  const el = e.target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

async function deleteSelection() {
  const { selection, edit, select, toast, undo } = useEditor.getState();
  if (selection.length === 0) return;
  const snap = await edit({ type: "deleteClips", clipIds: selection });
  if (snap) {
    select([]);
    toast({ kind: "info", text: selection.length === 1 ? "Clip deleted" : `${selection.length} clips deleted`, action: { label: "Undo", run: undo } });
  }
}

async function splitAtPlayhead() {
  const { snap, selection, timeUs, edit, toast } = useEditor.getState();
  if (!snap) return;
  const fps = snap.project.canvas.fps;
  const min = US / fps;
  const under = (s: number, d: number) => timeUs > s + min && timeUs < s + d - min;
  let targets = allClips(snap.project).filter((c) => selection.includes(c.id) && under(c.startUs, c.durationUs));
  if (targets.length === 0) targets = (snap.project.tracks.find((t) => t.id === MAIN_TRACK)?.clips ?? []).filter((c) => under(c.startUs, c.durationUs));
  if (targets.length === 0) {
    toast({ kind: "info", text: "Move the playhead over a clip to split it." });
    return;
  }
  for (const c of targets) await edit({ type: "splitClip", clipId: c.id, atUs: Math.round(timeUs) });
}

function useShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (isTyping(e) || useEditor.getState().exportOpen) return;
      const s = useEditor.getState();
      const mod = e.ctrlKey || e.metaKey;
      const fps = s.snap?.project.canvas.fps ?? 30;
      const key = e.key.toLowerCase();
      if (key === " ") {
        e.preventDefault();
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
        if (s.snap && projectDuration(s.snap.project) > 0) useEditor.setState({ exportOpen: true });
      } else if (!mod && key === "s") {
        e.preventDefault();
        splitAtPlayhead();
      } else if (key === "delete" || key === "backspace") {
        e.preventDefault();
        deleteSelection();
      } else if (key === "arrowleft" || key === "arrowright") {
        e.preventDefault();
        const dir = key === "arrowleft" ? -1 : 1;
        s.seek(s.timeUs + dir * (e.shiftKey ? US : US / fps));
      } else if (key === "home") {
        s.seek(0);
      } else if (key === "end") {
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

function useBackendEvents() {
  useEffect(() => {
    const offs: Promise<() => void>[] = [];
    offs.push(
      listen<Transport>("transport", (e) => {
        useEditor.setState({ playing: e.payload.playing, timeUs: e.payload.tUs });
      }),
    );
    offs.push(
      listen<{ revision: number; error: string | null }>("saved", (e) => {
        const snap = useEditor.getState().snap;
        if (e.payload.error) useEditor.setState({ saveState: "error" });
        else if (!snap || e.payload.revision >= snap.revision) useEditor.setState({ saveState: "saved" });
      }),
    );
    offs.push(
      listen<JobEvent>("job", (e) => {
        const job = e.payload;
        const { toast } = useEditor.getState();
        useEditor.setState({ jobs: { ...useEditor.getState().jobs, [job.id]: job } });
        if (job.kind === "captions" && job.status === "done") toast({ kind: "success", text: `Added ${job.output ?? "captions"}` });
        if (job.kind === "captions" && job.status === "failed") toast({ kind: "error", text: `Captions failed: ${job.message}` });
        if (job.kind === "audio" && job.status === "failed") toast({ kind: "error", text: `${job.label} failed: ${job.message}` });
      }),
    );
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

// Test hook for WebDriver runs; native file dialogs cannot be automated.
if (import.meta.env.DEV) Object.assign(window, { __capopen: { importPaths, store: useEditor, api } });

export default function App() {
  const snap = useEditor((s) => s.snap);
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
        <MediaPanel />
        <Preview />
        <Inspector />
      </div>
      <Timeline />
      <ExportDialog />
      <Toasts />
      <DragChip />
    </div>
  );
}
