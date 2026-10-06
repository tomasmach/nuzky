import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AudioLines, Captions, Film, Image as ImageIcon, Loader2, Plus, Trash2, Type, Upload, X } from "lucide-react";
import { api, errorText } from "../lib/api";
import { MAIN_TRACK, findClip, useEditor } from "../lib/store";
import { formatDuration } from "../lib/time";
import type { Asset, CaptionModel, TextStyle } from "../lib/types";
import { Button, Field, IconButton, ProgressBar } from "./ui";

export const MEDIA_EXTENSIONS = [
  "mp4", "mov", "m4v", "mkv", "webm", "avi", "mts", "mp3", "wav", "m4a", "aac", "flac", "ogg", "opus", "png", "jpg", "jpeg", "webp", "gif", "bmp",
];

/** Imports files; optionally places them on the timeline one after another. */
export async function importPaths(paths: string[], place?: { trackId: string | null; startUs: number | null }) {
  const { setSnap, toast, edit } = useEditor.getState();
  if (paths.length === 0) return;
  try {
    const res = await api.importMedia(paths);
    setSnap(res.snapshot);
    if (res.failed.length > 0) {
      const names = res.failed.map((f) => f.path.split(/[\\/]/).pop()).join(", ");
      toast({ kind: "error", text: `Could not import ${names}: ${res.failed[0].error}` });
    }
    if (place) {
      for (const id of res.added) await edit({ type: "addClip", assetId: id, startUs: place.startUs, trackId: place.trackId });
    }
  } catch (e) {
    toast({ kind: "error", text: errorText(e) });
  }
}

export async function pickAndImport() {
  const picked = await open({ multiple: true, filters: [{ name: "Video, audio and images", extensions: MEDIA_EXTENSIONS }] });
  if (picked) await importPaths(Array.isArray(picked) ? picked : [picked]);
}

function KindIcon({ kind, size = 14 }: { kind: Asset["kind"]; size?: number }) {
  if (kind === "audio") return <AudioLines size={size} />;
  if (kind === "image") return <ImageIcon size={size} />;
  return <Film size={size} />;
}

function MediaItem({ asset }: { asset: Asset }) {
  const thumb = useEditor((s) => s.thumbs[asset.id]);
  const job = useEditor((s) => s.jobs[`audio:${asset.id}`]);
  const { loadThumb, edit } = useEditor.getState();
  useEffect(() => loadThumb(asset.id), [asset.id, loadThumb]);

  const add = async () => {
    const snap = await edit({ type: "addClip", assetId: asset.id, startUs: useEditor.getState().timeUs, trackId: null });
    // Like CapCut, the playhead jumps past a clip added to the main track, so repeated adds append in order.
    const added = snap && findClip(snap.project, snap.select[0]);
    if (added && added.track.id === MAIN_TRACK) useEditor.getState().seek(added.clip.startUs + added.clip.durationUs);
  };
  const preparing = job?.status === "running";

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
    const start = { x: e.clientX, y: e.clientY };
    let dragging = false;
    const move = (ev: PointerEvent) => {
      if (!dragging && Math.hypot(ev.clientX - start.x, ev.clientY - start.y) > 4) dragging = true;
      if (dragging) useEditor.setState({ assetDrag: { assetId: asset.id, x: ev.clientX, y: ev.clientY } });
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const drag = useEditor.getState().assetDrag;
      useEditor.setState({ assetDrag: null });
      if (!dragging || !drag) return;
      const target = dropResolver?.(ev.clientX, ev.clientY);
      if (target) edit({ type: "addClip", assetId: asset.id, startUs: target.startUs, trackId: target.trackId });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div
      onPointerDown={onPointerDown}
      onDoubleClick={add}
      className="group relative flex cursor-grab flex-col gap-1 rounded-md p-1 hover:bg-raised active:cursor-grabbing"
      title={`${asset.name}\nDrag onto the timeline, or press + to add at the playhead`}
    >
      <div className="relative aspect-video overflow-hidden rounded bg-bg">
        {asset.kind === "audio" ? (
          <div className="flex h-full items-center justify-center bg-clip-audio/50 text-muted">
            <AudioLines size={28} />
          </div>
        ) : thumb === undefined ? (
          <div className="skeleton h-full w-full" />
        ) : thumb ? (
          <img src={thumb} alt="" className="h-full w-full object-contain" draggable={false} />
        ) : (
          <div className="flex h-full items-center justify-center text-subtle">
            <KindIcon kind={asset.kind} size={24} />
          </div>
        )}
        <span className="tabular absolute bottom-1 right-1 rounded bg-black/70 px-1 text-[10px] text-fg">
          {asset.kind === "image" ? "Image" : formatDuration(asset.durationUs)}
        </span>
        <button
          type="button"
          aria-label={`Add ${asset.name} at playhead`}
          title="Add at playhead"
          onClick={add}
          className="absolute right-1 top-1 flex h-7 w-7 items-center justify-center rounded-md bg-accent text-black opacity-0 shadow transition-opacity duration-[120ms] focus:opacity-100 group-hover:opacity-100"
        >
          <Plus size={16} />
        </button>
        {preparing && (
          <div className="absolute inset-x-1 bottom-1 mr-12">
            <ProgressBar value={job.progress} />
          </div>
        )}
      </div>
      <div className="flex items-center gap-1 px-0.5 text-[12px] text-muted">
        <KindIcon kind={asset.kind} size={12} />
        <span className="truncate">{asset.name}</span>
      </div>
    </div>
  );
}

export let dropResolver: ((x: number, y: number) => { trackId: string | null; startUs: number } | null) | null = null;
export function setDropResolver(fn: typeof dropResolver) {
  dropResolver = fn;
}

function MediaTab() {
  const assets = useEditor((s) => s.snap?.project.assets ?? []);
  const edit = useEditor((s) => s.edit);
  const [confirm, setConfirm] = useState<string | null>(null);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center gap-2 p-3">
        <Button variant="primary" className="flex-1" onClick={pickAndImport}>
          <Upload size={15} /> Import media
        </Button>
      </div>
      {assets.length === 0 ? (
        <div className="m-3 mt-0 flex flex-1 flex-col items-center justify-center gap-2 rounded-lg border border-dashed border-line p-6 text-center">
          <Film size={28} className="text-subtle" />
          <p className="text-[13px] text-fg">No media yet</p>
          <p className="text-[12px] text-muted">Import videos, music or images, or drop files anywhere on the window.</p>
        </div>
      ) : (
        <div className="grid min-h-0 grid-cols-2 content-start gap-1 overflow-y-auto px-2 pb-3">
          {assets.map((a) => (
            <div key={a.id} className="relative">
              <MediaItem asset={a} />
              {confirm === a.id ? (
                <div className="absolute inset-1 z-10 flex flex-col items-center justify-center gap-2 rounded bg-panel/95 p-2 text-center text-[12px]">
                  <span>Remove from project? Its clips are removed too.</span>
                  <span className="flex gap-1">
                    <Button variant="danger" className="h-7" onClick={() => edit({ type: "removeAsset", assetId: a.id })}>
                      Remove
                    </Button>
                    <Button className="h-7" onClick={() => setConfirm(null)}>
                      Keep
                    </Button>
                  </span>
                </div>
              ) : (
                <IconButton
                  label={`Remove ${a.name}`}
                  onClick={() => setConfirm(a.id)}
                  className="absolute left-2 top-2 h-6 w-6 bg-black/60 opacity-0 hover:opacity-100 focus:opacity-100"
                >
                  <Trash2 size={13} />
                </IconButton>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export const TEXT_PRESETS: { name: string; text: string; style: TextStyle }[] = [
  { name: "Classic", text: "Your text", style: { fontSize: 84, color: "#ffffff", bold: true, strokeWidth: 7, strokeColor: "#000000", background: null } },
  { name: "Title", text: "BIG TITLE", style: { fontSize: 130, color: "#ffffff", bold: true, strokeWidth: 0, strokeColor: "#000000", background: null } },
  { name: "Label", text: "Label", style: { fontSize: 72, color: "#111111", bold: true, strokeWidth: 0, strokeColor: "#000000", background: "#ffffffee" } },
  { name: "Highlight", text: "Highlight", style: { fontSize: 84, color: "#111111", bold: true, strokeWidth: 0, strokeColor: "#000000", background: "#ffd400ee" } },
  { name: "Neon", text: "Neon", style: { fontSize: 96, color: "#9cffd9", bold: true, strokeWidth: 5, strokeColor: "#0b6b4f", background: null } },
  { name: "Plain", text: "Plain text", style: { fontSize: 64, color: "#ffffff", bold: false, strokeWidth: 0, strokeColor: "#000000", background: null } },
];

export function PresetSwatch({ style, label }: { style: TextStyle; label: string }) {
  return (
    <span
      className="inline-block max-w-full truncate rounded px-1.5 text-[15px] leading-6"
      style={{
        color: style.color,
        fontWeight: style.bold ? 800 : 400,
        background: style.background ?? undefined,
        WebkitTextStroke: style.strokeWidth > 0 ? `${Math.min(2, style.strokeWidth / 4)}px ${style.strokeColor}` : undefined,
        paintOrder: "stroke fill",
      }}
    >
      {label}
    </span>
  );
}

function TextTab() {
  const edit = useEditor((s) => s.edit);
  return (
    <div className="flex flex-col gap-2 overflow-y-auto p-3">
      <p className="text-[12px] text-muted">Adds a text clip at the playhead.</p>
      <div className="grid grid-cols-2 gap-2">
        {TEXT_PRESETS.map((p) => (
          <button
            key={p.name}
            type="button"
            onClick={() => edit({ type: "addText", startUs: useEditor.getState().timeUs, text: p.text, style: p.style })}
            className="flex h-16 flex-col items-center justify-center gap-1 rounded-md border border-line bg-[#2b3036] px-2 hover:border-accent"
          >
            <PresetSwatch style={p.style} label={p.name === "Title" ? "TITLE" : p.name} />
          </button>
        ))}
      </div>
    </div>
  );
}

const LANGUAGES = [
  ["auto", "Detect automatically"],
  ["cs", "Czech"],
  ["sk", "Slovak"],
  ["en", "English"],
  ["de", "German"],
  ["pl", "Polish"],
  ["es", "Spanish"],
  ["fr", "French"],
  ["it", "Italian"],
  ["uk", "Ukrainian"],
];

export const CAPTION_STYLES: { name: string; style: TextStyle }[] = [
  { name: "Outline", style: { fontSize: 70, color: "#ffffff", bold: true, strokeWidth: 7, strokeColor: "#000000", background: null } },
  { name: "Yellow", style: { fontSize: 74, color: "#ffe14d", bold: true, strokeWidth: 7, strokeColor: "#000000", background: null } },
  { name: "Box", style: { fontSize: 62, color: "#ffffff", bold: true, strokeWidth: 0, strokeColor: "#000000", background: "#000000b3" } },
  { name: "Clean", style: { fontSize: 60, color: "#ffffff", bold: false, strokeWidth: 3, strokeColor: "#00000099", background: null } },
];

function CaptionsTab() {
  const [models, setModels] = useState<CaptionModel[]>([]);
  const [model, setModel] = useState("small");
  const [language, setLanguage] = useState("cs");
  const [styleIdx, setStyleIdx] = useState(0);
  const jobs = useEditor((s) => s.jobs);
  const hasCaptions = useEditor((s) => s.snap?.project.tracks.some((t) => t.kind === "text" && t.name === "Captions") ?? false);
  const job = Object.values(jobs).find((j) => j.kind === "captions" && j.status === "running");
  const lastJob = Object.values(jobs).filter((j) => j.kind === "captions").pop();

  useEffect(() => {
    api.captionModels().then(setModels);
  }, [job?.id, lastJob?.status]);

  const start = async () => {
    try {
      await api.startCaptions(model, language, CAPTION_STYLES[styleIdx].style);
    } catch (e) {
      useEditor.getState().toast({ kind: "error", text: errorText(e) });
    }
  };
  const selected = models.find((m) => m.id === model);

  return (
    <div className="flex flex-col gap-4 overflow-y-auto p-3">
      <p className="text-[12px] text-muted">Recognises speech in your video clips and adds captions on their own track. Runs on this computer; nothing is uploaded.</p>
      <Field label="Spoken language">
        <select value={language} onChange={(e) => setLanguage(e.target.value)} className="h-8 rounded-md border border-line bg-raised px-2 text-[13px]">
          {LANGUAGES.map(([id, name]) => (
            <option key={id} value={id}>
              {name}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Accuracy" hint={selected && !selected.downloaded ? `Downloads a ${selected.sizeMb} MB speech model on first use.` : undefined}>
        <select value={model} onChange={(e) => setModel(e.target.value)} className="h-8 rounded-md border border-line bg-raised px-2 text-[13px]">
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.label} ({m.sizeMb} MB){m.downloaded ? "" : " · download"}
            </option>
          ))}
        </select>
      </Field>
      <div className="flex flex-col gap-1.5">
        <span className="text-[12px] text-muted">Style</span>
        <div className="grid grid-cols-2 gap-2">
          {CAPTION_STYLES.map((s, i) => (
            <button
              key={s.name}
              type="button"
              aria-pressed={i === styleIdx}
              onClick={() => setStyleIdx(i)}
              className={`flex h-12 items-center justify-center rounded-md border bg-[#2b3036] ${i === styleIdx ? "border-accent ring-1 ring-accent" : "border-line hover:border-muted"}`}
            >
              <PresetSwatch style={s.style} label={s.name} />
            </button>
          ))}
        </div>
      </div>
      {job ? (
        <div className="flex flex-col gap-2 rounded-md border border-line bg-raised p-3" role="status">
          <div className="flex items-center justify-between text-[12px]">
            <span className="flex items-center gap-1.5 text-fg">
              <Loader2 size={13} className="animate-spin text-accent" />
              {job.phase ?? "Starting"}
            </span>
            <span className="tabular text-muted">{Math.round(job.progress * 100)}%</span>
          </div>
          <ProgressBar value={job.progress} />
          <Button variant="ghost" className="self-end" onClick={() => api.cancelJob(job.id)}>
            <X size={14} /> Cancel
          </Button>
        </div>
      ) : (
        <Button variant="primary" onClick={start}>
          <Captions size={15} /> {hasCaptions ? "Regenerate captions" : "Generate captions"}
        </Button>
      )}
      {!job && hasCaptions && <p className="-mt-2 text-[11px] text-subtle">Regenerating replaces the current Captions track.</p>}
      {!job && lastJob?.status === "failed" && (
        <p className="flex gap-1.5 text-[12px] text-danger" role="alert">
          {lastJob.message}
        </p>
      )}
    </div>
  );
}

const TABS = [
  { id: "media", label: "Media", icon: Film },
  { id: "text", label: "Text", icon: Type },
  { id: "captions", label: "Captions", icon: Captions },
] as const;

export function MediaPanel() {
  const [tab, setTab] = useState<(typeof TABS)[number]["id"]>("media");
  return (
    <aside className="flex w-[300px] shrink-0 flex-col border-r border-line bg-panel">
      <div className="flex border-b border-line" role="tablist">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            onClick={() => setTab(t.id)}
            className={`flex flex-1 flex-col items-center gap-0.5 py-2 text-[11px] transition-colors duration-[120ms] ${
              tab === t.id ? "text-accent shadow-[inset_0_-2px_0_var(--color-accent)]" : "text-muted hover:text-fg"
            }`}
          >
            <t.icon size={17} />
            {t.label}
          </button>
        ))}
      </div>
      {tab === "media" && <MediaTab />}
      {tab === "text" && <TextTab />}
      {tab === "captions" && <CaptionsTab />}
    </aside>
  );
}
