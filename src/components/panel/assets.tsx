import { open } from "@tauri-apps/plugin-dialog";
import { AudioLines, Film, Image as ImageIcon, Plus } from "lucide-react";
import { api, errorText } from "../../lib/api";
import { followPointer } from "../../lib/drag";
import { MAIN_TRACK, aiLocked, allClips, currentEpoch, findClip, undoAction, useEditor } from "../../lib/store";
import type { Asset } from "../../lib/types";
import { lockedProps, useLockReason } from "../ui";

export const AUDIO_EXTENSIONS = ["mp3", "wav", "m4a", "aac", "flac", "ogg", "opus"];
const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];
export const MEDIA_EXTENSIONS = ["mp4", "mov", "m4v", "mkv", "webm", "avi", "mts", ...AUDIO_EXTENSIONS, ...IMAGE_EXTENSIONS];

const fileName = (path: string) => path.split(/[\\/]/).pop() || path;
const extension = (path: string) => path.split(".").pop()?.toLowerCase() ?? "";

let importKey = 0;

/**
 * One short line for the files that failed: the name only, never the full path or FFmpeg's wording.
 * The engine's error ends with the reason, after "Cannot open <path>: ".
 */
export function importFailures(failed: { path: string; error: string }[]) {
  // The reason is what follows the path, so a file name such as "has no video.mp4" cannot match.
  const reason = (f: { path: string; error: string }) => {
    const at = f.error.lastIndexOf(f.path);
    return (at >= 0 ? f.error.slice(at + f.path.length) : f.error).replace(/^:\s*/, "");
  };
  const undecodable = (f: { path: string; error: string }) => /Invalid data found|has no video or audio/.test(reason(f));
  const bad = failed.filter(undecodable).map((f) => fileName(f.path));
  const other = failed.filter((f) => !undecodable(f)).map((f) => `Could not import ${fileName(f.path)}: ${reason(f).split(": ").pop()}`);
  const lead = bad.length === 1 ? `${bad[0]} is not a video, audio or image file` : bad.length > 1 ? `${bad.join(", ")} are not video, audio or image files` : null;
  return [lead, ...other].filter(Boolean).join(". ");
}

/** Imports files; optionally places them on the timeline one after another. */
export async function importPaths(paths: string[], place?: { trackId: string | null; startUs: number | null }) {
  const { setSnap, toast, edit } = useEditor.getState();
  if (paths.length === 0 || aiLocked()) return;
  const epoch = currentEpoch();
  // Placeholders show at once, while the files are probed.
  const kind = (p: string): Asset["kind"] => (AUDIO_EXTENSIONS.includes(extension(p)) ? "audio" : IMAGE_EXTENSIONS.includes(extension(p)) ? "image" : "video");
  const pending = paths.map((p) => ({ key: ++importKey, name: fileName(p), kind: kind(p) }));
  useEditor.setState({ importing: [...useEditor.getState().importing, ...pending] });
  const done = () => useEditor.setState({ importing: useEditor.getState().importing.filter((i) => !pending.includes(i)) });
  try {
    const res = await api.importMedia(paths, epoch);
    setSnap(res.snapshot);
    done();
    if (res.failed.length > 0) toast({ kind: "error", text: importFailures(res.failed) });
    // Placing waits for the import; another project may have opened meanwhile.
    if (place && currentEpoch() === epoch) {
      let startUs = place.startUs;
      for (const id of res.added) {
        if (currentEpoch() !== epoch) break;
        const snap = await edit({ type: "addClip", assetId: id, startUs, trackId: place.trackId });
        const added = snap && findClip(snap.project, snap.select[0]);
        if (added) startUs = added.clip.startUs + added.clip.durationUs;
      }
    }
  } catch (e) {
    toast({ kind: "error", text: errorText(e) });
  } finally {
    done();
  }
}

/** A file being imported: the name and a skeleton where its thumbnail will be. */
export function ImportPlaceholder({ name, compact }: { name: string; compact?: boolean }) {
  if (compact)
    return (
      <div className="flex items-center gap-2 rounded-xl p-1.5" role="status" aria-label={`Importing ${name}`}>
        <div className="skeleton h-10 w-10 shrink-0 rounded-md" />
        <span className="min-w-0 flex-1 truncate text-[13px] text-muted">{name}</span>
      </div>
    );
  return (
    <div className="flex min-w-0 flex-col gap-1.5" role="status" aria-label={`Importing ${name}`}>
      <div className="skeleton aspect-square rounded-[10px]" />
      <span className="truncate text-[11px] leading-[13px] text-muted">{name}</span>
    </div>
  );
}

/** The calm full-width tile on top of Media and Audio that opens the file picker. */
export function ImportTile({ label, hint, shortcut, onClick, className = "" }: { label: string; hint?: string; shortcut?: string; onClick: () => void; className?: string }) {
  const lock = useLockReason();
  return (
    <button
      type="button"
      aria-keyshortcuts={shortcut}
      onClick={onClick}
      {...lockedProps(lock)}
      className={`flex h-[46px] shrink-0 items-center justify-center gap-2.5 rounded-xl bg-white/[.055] text-[13px] font-semibold text-fg shadow-[inset_0_0_0_1px_rgb(255_255_255/.07)] transition-colors duration-[120ms] ease-out hover:bg-white/[.09] aria-disabled:cursor-not-allowed aria-disabled:opacity-40 aria-disabled:hover:bg-white/[.055] ${className}`}
    >
      <span className="flex h-7 w-7 items-center justify-center rounded-full bg-accent-strong text-white shadow-[inset_0_1px_0_rgb(255_255_255/.2)]">
        <Plus size={16} />
      </span>
      {label}
      {hint && (
        <kbd aria-hidden className="ml-0.5 rounded-[5px] bg-white/[.055] px-[5px] py-0.5 font-sans text-[11px] leading-[13px] font-medium text-muted">
          {hint}
        </kbd>
      )}
    </button>
  );
}

export async function pickAndImport(audioOnly = false) {
  if (aiLocked()) return;
  const picked = await open({
    multiple: true,
    filters: [audioOnly ? { name: "Audio", extensions: AUDIO_EXTENSIONS } : { name: "Video, audio and images", extensions: MEDIA_EXTENSIONS }],
  });
  if (picked) await importPaths(Array.isArray(picked) ? picked : [picked]);
}

export async function addAtPlayhead(assetId: string) {
  const { edit, seek } = useEditor.getState();
  const snap = await edit({ type: "addClip", assetId, startUs: useEditor.getState().timeUs, trackId: null });
  // Like CapCut, the playhead jumps past a clip added to the main track, so repeated adds append in order.
  const added = snap && findClip(snap.project, snap.select[0]);
  if (added && added.track.id === MAIN_TRACK) seek(added.clip.startUs + added.clip.durationUs);
}

/** Puts a video or image in a small rounded window in the corner, from the playhead, above the main track. */
export function addPictureInPicture(assetId: string) {
  const { edit, timeUs } = useEditor.getState();
  return edit({ type: "addPictureInPicture", assetId, startUs: Math.round(timeUs), durationUs: null });
}

export let dropResolver: ((x: number, y: number) => { trackId: string | null; startUs: number } | null) | null = null;
export function setDropResolver(fn: typeof dropResolver) {
  dropResolver = fn;
}

/** Pointer handler that drags an asset from the panel onto a timeline track. */
export function assetDragHandler(assetId: string) {
  return (e: React.PointerEvent) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
    const start = { x: e.clientX, y: e.clientY };
    let dragging = false;
    const move = (ev: PointerEvent) => {
      if (!dragging && Math.hypot(ev.clientX - start.x, ev.clientY - start.y) > 4) dragging = true;
      if (dragging) useEditor.setState({ assetDrag: { assetId, x: ev.clientX, y: ev.clientY } });
    };
    const up = (ev: PointerEvent) => {
      const drag = useEditor.getState().assetDrag;
      useEditor.setState({ assetDrag: null });
      if (!dragging || !drag) return;
      const target = dropResolver?.(ev.clientX, ev.clientY);
      if (target) useEditor.getState().edit({ type: "addClip", assetId, startUs: target.startUs, trackId: target.trackId });
    };
    // A cancelled drag drops nothing.
    followPointer({ move, up, cancel: () => useEditor.setState({ assetDrag: null }) });
  };
}

export function KindIcon({ kind, size = 14 }: { kind: Asset["kind"]; size?: number }) {
  if (kind === "audio") return <AudioLines size={size} />;
  if (kind === "image") return <ImageIcon size={size} />;
  return <Film size={size} />;
}

/** Removes an asset and its clips at once; the toast's Undo brings both back. */
export async function removeAsset(asset: Asset) {
  const { snap: before, edit, toast } = useEditor.getState();
  const n = before ? allClips(before.project).filter((c) => c.content.type === "media" && c.content.assetId === asset.id).length : 0;
  const snap = await edit({ type: "removeAsset", assetId: asset.id });
  if (snap) toast({ kind: "info", text: n > 0 ? `Removed ${asset.name} and its ${n === 1 ? "clip" : `${n} clips`}` : `Removed ${asset.name}`, action: undoAction(snap) });
}
