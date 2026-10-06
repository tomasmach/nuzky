import { create } from "zustand";
import { api, errorText } from "./api";
import { clipOffset, keyframeTolerance, transformAt, upsertKeyframe } from "./keyframes";
import { US } from "./time";
import type { Clip, EditCmd, Filmstrip, JobEvent, Project, Snapshot, TextStyle, Track, Transform, Transition } from "./types";

export interface Toast {
  id: number;
  kind: "info" | "success" | "error";
  text: string;
  action?: { label: string; run: () => void };
}

/** A media item being dragged from the media panel onto the timeline. */
export interface AssetDrag {
  assetId: string;
  x: number;
  y: number;
}

export type PanelTab = "media" | "audio" | "text" | "captions" | "transitions" | "filters";

interface EditorState {
  snap: Snapshot | null;
  previewUrl: string;
  selection: string[];
  /** Clip whose incoming cut (transition) is selected; exclusive with `selection`. */
  cut: string | null;
  playing: boolean;
  timeUs: number;
  /** Timeline zoom in pixels per second. */
  zoom: number;
  jobs: Record<string, JobEvent>;
  toasts: Toast[];
  saveState: "saved" | "saving" | "error";
  engineError: string | null;
  thumbs: Record<string, string | null>;
  filmstrips: Record<string, Filmstrip | null>;
  waveforms: Record<string, number[] | null>;
  assetDrag: AssetDrag | null;
  exportOpen: boolean;
  exportJobId: string | null;
  panelTab: PanelTab;

  setSnap: (snap: Snapshot, keepSelection?: boolean) => void;
  edit: (cmd: EditCmd, coalesce?: string) => Promise<Snapshot | null>;
  undo: () => Promise<void>;
  redo: () => Promise<void>;
  select: (ids: string[]) => void;
  selectCut: (clipId: string | null) => void;
  seek: (tUs: number) => void;
  togglePlay: () => void;
  setZoom: (zoom: number) => void;
  toast: (t: Omit<Toast, "id">) => void;
  dismissToast: (id: number) => void;
  loadThumb: (assetId: string) => void;
  loadFilmstrip: (assetId: string) => void;
  loadWaveform: (assetId: string, force?: boolean) => void;
}

let toastId = 0;
const pending = new Set<string>();

export const useEditor = create<EditorState>((set, get) => ({
  snap: null,
  previewUrl: "",
  selection: [],
  cut: null,
  playing: false,
  timeUs: 0,
  zoom: 60,
  jobs: {},
  toasts: [],
  saveState: "saved",
  engineError: null,
  thumbs: {},
  filmstrips: {},
  waveforms: {},
  assetDrag: null,
  exportOpen: false,
  exportJobId: null,
  panelTab: "media",

  setSnap: (snap, keepSelection = true) => {
    const ids = new Set(allClips(snap.project).map((c) => c.id));
    const kept = keepSelection ? get().selection.filter((id) => ids.has(id)) : [];
    const cut = get().cut;
    set({
      snap,
      selection: snap.select.length > 0 ? snap.select : kept,
      cut: snap.select.length === 0 && keepSelection && cut && mainCuts(snap.project).some((c) => c.clipId === cut) ? cut : null,
      saveState: "saving",
      timeUs: Math.min(get().timeUs, projectDuration(snap.project)),
    });
  },

  edit: async (cmd, coalesce) => {
    try {
      const snap = await api.applyEdit(cmd, coalesce);
      get().setSnap(snap);
      return snap;
    } catch (e) {
      get().toast({ kind: "error", text: errorText(e) });
      return null;
    }
  },

  undo: async () => {
    if (!get().snap?.canUndo) return;
    get().setSnap(await api.undo());
  },

  redo: async () => {
    if (!get().snap?.canRedo) return;
    get().setSnap(await api.redo());
  },

  select: (ids) => set({ selection: ids, cut: null }),

  selectCut: (clipId) => set({ cut: clipId, selection: [] }),

  seek: (tUs) => {
    const snap = get().snap;
    const end = snap ? projectDuration(snap.project) : 0;
    const t = Math.max(0, Math.min(end, tUs));
    set({ timeUs: t });
    api.seek(t);
  },

  togglePlay: () => {
    if (get().playing) api.pause();
    else api.play();
  },

  setZoom: (zoom) => set({ zoom: Math.max(4, Math.min(600, zoom)) }),

  toast: (t) => {
    const id = ++toastId;
    set({ toasts: [...get().toasts.slice(-3), { ...t, id }] });
    window.setTimeout(() => get().dismissToast(id), t.kind === "error" ? 8000 : t.action ? 7000 : 5000);
  },

  dismissToast: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),

  loadThumb: (assetId) => {
    const key = `thumb:${assetId}`;
    if (assetId in get().thumbs || pending.has(key)) return;
    pending.add(key);
    api
      .thumbnail(assetId)
      .then((url) => set({ thumbs: { ...get().thumbs, [assetId]: url } }))
      .catch(() => set({ thumbs: { ...get().thumbs, [assetId]: null } }))
      .finally(() => pending.delete(key));
  },

  loadFilmstrip: (assetId) => {
    const key = `strip:${assetId}`;
    if (assetId in get().filmstrips || pending.has(key)) return;
    pending.add(key);
    api
      .filmstrip(assetId)
      .then((strip) => set({ filmstrips: { ...get().filmstrips, [assetId]: strip } }))
      .catch(() => set({ filmstrips: { ...get().filmstrips, [assetId]: null } }))
      .finally(() => pending.delete(key));
  },

  loadWaveform: (assetId, force = false) => {
    const key = `wave:${assetId}`;
    if ((!force && get().waveforms[assetId]) || pending.has(key)) return;
    pending.add(key);
    api
      .waveform(assetId)
      .then((peaks) => peaks && set({ waveforms: { ...get().waveforms, [assetId]: peaks } }))
      .finally(() => pending.delete(key));
  },
}));

export function allClips(project: Project): Clip[] {
  return project.tracks.flatMap((t) => t.clips);
}

export function projectDuration(project: Project): number {
  return allClips(project).reduce((m, c) => Math.max(m, c.startUs + c.durationUs), 0);
}

export function findClip(project: Project, id: string): { clip: Clip; track: Track } | null {
  for (const track of project.tracks) {
    const clip = track.clips.find((c) => c.id === id);
    if (clip) return { clip, track };
  }
  return null;
}

/** Display order: overlays and text above the main track, audio below. */
export function displayTracks(project: Project): Track[] {
  const visual = project.tracks.filter((t) => t.kind !== "audio").reverse();
  const audio = project.tracks.filter((t) => t.kind === "audio");
  return [...visual, ...audio];
}

export const MAIN_TRACK = "main";

export function isCaptionTrack(track: Track) {
  return track.kind === "text" && track.name === "Captions";
}

export function mainClips(project: Project): Clip[] {
  return project.tracks.find((t) => t.id === MAIN_TRACK)?.clips ?? [];
}

/** Cuts between main-track clips; each belongs to the clip after it. */
export function mainCuts(project: Project): { clipId: string; atUs: number; transition: Transition | null }[] {
  return mainClips(project)
    .slice(1)
    .map((c) => ({ clipId: c.id, atUs: c.startUs, transition: c.transitionIn }));
}

export type Cut = ReturnType<typeof mainCuts>[number];

/**
 * Cut that transition presets apply to: the selected cut, else the cut before the selected
 * main-track clip, else the cut nearest the playhead.
 */
export function transitionTarget(project: Project, selection: string[], cut: string | null, timeUs: number): Cut | null {
  const cuts = mainCuts(project);
  if (cuts.length === 0) return null;
  const picked = cut ?? (selection.length === 1 ? selection[0] : null);
  const own = picked ? cuts.find((c) => c.clipId === picked) : undefined;
  if (own) return own;
  return cuts.reduce((best, c) => (Math.abs(c.atUs - timeUs) < Math.abs(best.atUs - timeUs) ? c : best));
}

/** Transform the renderer uses at the playhead, keyframes included. */
export function transformAtPlayhead(clip: Clip, timeUs: number): Transform {
  return transformAt(clip, clip.content.transform, clipOffset(clip, timeUs));
}

/**
 * Writes a transform the way CapCut does: a clip with keyframes gets a keyframe at the
 * playhead, a clip without keyframes changes its single transform.
 */
export function setClipTransform(clip: Clip, transform: Transform, coalesce?: string) {
  const { edit, timeUs, snap } = useEditor.getState();
  if (clip.keyframes.length === 0) return edit({ type: "updateClip", clipId: clip.id, transform }, coalesce);
  const tol = keyframeTolerance(snap?.project.canvas.fps ?? 30);
  return edit({ type: "setKeyframes", clipId: clip.id, keyframes: upsertKeyframe(clip, clipOffset(clip, timeUs), transform, tol) }, coalesce);
}

export async function deleteSelection() {
  const { selection, cut, edit, select, selectCut, toast, undo } = useEditor.getState();
  if (cut) {
    if (await edit({ type: "setTransition", clipId: cut, transition: null })) {
      selectCut(null);
      toast({ kind: "info", text: "Transition removed", action: { label: "Undo", run: undo } });
    }
    return;
  }
  if (selection.length === 0) return;
  if (await edit({ type: "deleteClips", clipIds: selection })) {
    select([]);
    toast({ kind: "info", text: selection.length === 1 ? "Clip deleted" : `${selection.length} clips deleted`, action: { label: "Undo", run: undo } });
  }
}

/** Selected clips under the playhead, or the main-track clip under it when none is selected. */
export function splitTargets(project: Project, selection: string[], timeUs: number): Clip[] {
  const min = US / project.canvas.fps;
  const under = (c: Clip) => timeUs > c.startUs + min && timeUs < c.startUs + c.durationUs - min;
  const sel = allClips(project).filter((c) => selection.includes(c.id) && under(c));
  return sel.length > 0 ? sel : mainClips(project).filter(under);
}

export async function splitAtPlayhead() {
  const { snap, selection, timeUs, edit, toast } = useEditor.getState();
  if (!snap) return;
  const targets = splitTargets(snap.project, selection, timeUs);
  if (targets.length === 0) {
    toast({ kind: "info", text: "Move the playhead over a clip to split it." });
    return;
  }
  for (const c of targets) await edit({ type: "splitClip", clipId: c.id, atUs: Math.round(timeUs) });
}

export async function duplicateSelection() {
  const { selection, edit, select } = useEditor.getState();
  const copies: string[] = [];
  for (const id of selection) {
    const snap = await edit({ type: "duplicateClip", clipId: id });
    if (snap) copies.push(...snap.select);
  }
  if (copies.length > 0) select(copies);
}

/** Why a clip's sound cannot be detached, or null when it can. */
export function detachBlocker(project: Project, clip: Clip): string | null {
  const c = clip.content;
  if (c.type !== "media") return "Text clips have no sound";
  const asset = project.assets.find((a) => a.id === c.assetId);
  if (!asset || asset.kind !== "video" || findClip(project, clip.id)?.track.kind === "audio") return "Only video clips have sound to detach";
  if (!asset.hasAudio) return "This video has no sound";
  if (c.volume <= 0) return "Sound is already detached or muted";
  return null;
}

export async function detachAudio(clipId: string) {
  const { edit, toast, undo } = useEditor.getState();
  if (await edit({ type: "detachAudio", clipId })) toast({ kind: "info", text: "Audio moved to its own track", action: { label: "Undo", run: undo } });
}

/** Restyles every caption clip as one undo step. */
export async function applyCaptionStyle(style: TextStyle) {
  const { snap, edit } = useEditor.getState();
  const track = snap?.project.tracks.find(isCaptionTrack);
  if (!track) return;
  const key = `captions-style:${Date.now()}`;
  for (const c of track.clips) await edit({ type: "updateClip", clipId: c.id, style }, key);
}

export function openExport() {
  const { snap, exportJobId, jobs } = useEditor.getState();
  const running = exportJobId && jobs[exportJobId]?.status === "running";
  if (running || (snap && projectDuration(snap.project) > 0)) useEditor.setState({ exportOpen: true });
}
