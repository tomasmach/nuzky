import { create } from "zustand";
import { api, errorText } from "./api";
import type { Clip, EditCmd, JobEvent, Project, Snapshot, Track } from "./types";

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

interface EditorState {
  snap: Snapshot | null;
  previewUrl: string;
  selection: string[];
  playing: boolean;
  timeUs: number;
  /** Timeline zoom in pixels per second. */
  zoom: number;
  jobs: Record<string, JobEvent>;
  toasts: Toast[];
  saveState: "saved" | "saving" | "error";
  engineError: string | null;
  thumbs: Record<string, string | null>;
  waveforms: Record<string, number[] | null>;
  assetDrag: AssetDrag | null;
  exportOpen: boolean;

  setSnap: (snap: Snapshot, keepSelection?: boolean) => void;
  edit: (cmd: EditCmd, coalesce?: string) => Promise<Snapshot | null>;
  undo: () => Promise<void>;
  redo: () => Promise<void>;
  select: (ids: string[]) => void;
  seek: (tUs: number) => void;
  togglePlay: () => void;
  setZoom: (zoom: number) => void;
  toast: (t: Omit<Toast, "id">) => void;
  dismissToast: (id: number) => void;
  loadThumb: (assetId: string) => void;
  loadWaveform: (assetId: string, force?: boolean) => void;
}

let toastId = 0;
const pending = new Set<string>();

export const useEditor = create<EditorState>((set, get) => ({
  snap: null,
  previewUrl: "",
  selection: [],
  playing: false,
  timeUs: 0,
  zoom: 60,
  jobs: {},
  toasts: [],
  saveState: "saved",
  engineError: null,
  thumbs: {},
  waveforms: {},
  assetDrag: null,
  exportOpen: false,

  setSnap: (snap, keepSelection = true) => {
    const ids = new Set(allClips(snap.project).map((c) => c.id));
    const kept = keepSelection ? get().selection.filter((id) => ids.has(id)) : [];
    set({
      snap,
      selection: snap.select.length > 0 ? snap.select : kept,
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

  select: (ids) => set({ selection: ids }),

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
    window.setTimeout(() => get().dismissToast(id), t.kind === "error" ? 8000 : 5000);
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
