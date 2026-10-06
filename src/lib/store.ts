import { create } from "zustand";
import { api, errorText } from "./api";
import { clipOffset, keyframeTolerance, transformAt, upsertKeyframe } from "./keyframes";
import { US } from "./time";
import type { Clip, EditCmd, Filmstrip, JobEvent, Project, Snapshot, TextStyle, TimeRange, Track, Transform, Transition } from "./types";

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

export type PanelTab = "media" | "audio" | "text" | "captions" | "transcript" | "transitions" | "filters";

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
  /** Extra space under the toasts, e.g. for the transcript's Delete bar while it shows. */
  toastLift: number;
  saveState: "saved" | "saving" | "error";
  engineError: string | null;
  thumbs: Record<string, string | null>;
  filmstrips: Record<string, Filmstrip | null>;
  waveforms: Record<string, number[] | null>;
  assetDrag: AssetDrag | null;
  exportOpen: boolean;
  exportJobId: string | null;
  panelTab: PanelTab;
  ratioOpen: boolean;
  /** Audio tracks the user set to stay in place (true) or to be cut with the video (false). */
  keepTracks: Record<string, boolean>;

  setSnap: (snap: Snapshot, keepSelection?: boolean) => void;
  /**
   * Queues an edit. A function is called with the latest confirmed project right before
   * sending, so edits fired before the previous snapshot arrives build on it instead of
   * overwriting it. A list is applied all or nothing, as one undo step. Resolves to null
   * when the edit failed or the builder had nothing to do.
   */
  edit: (input: EditInput, coalesce?: string) => Promise<Snapshot | null>;
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

export type EditInput = EditCmd | EditCmd[] | ((project: Project) => EditCmd | EditCmd[] | null);

let toastId = 0;
const pending = new Set<string>();

/**
 * Edits, undo and redo run one at a time in the order they were made. An entry with the same
 * coalesce key as the unsent one before it replaces that one: both set absolute values and
 * would merge into one undo step anyway, so a fast slider drag never builds a backlog.
 */
interface QueuedEdit {
  run: () => Promise<Snapshot | null>;
  key: string | null;
  waiters: ((snap: Snapshot | null) => void)[];
}
const queue: QueuedEdit[] = [];
let draining = false;

function enqueue(run: () => Promise<Snapshot | null>, key: string | null = null): Promise<Snapshot | null> {
  return new Promise((resolve) => {
    const last = queue[queue.length - 1];
    if (key && last?.key === key) {
      last.run = run;
      last.waiters.push(resolve);
    } else queue.push({ run, key, waiters: [resolve] });
    void drain();
  });
}

async function drain() {
  if (draining) return;
  draining = true;
  while (queue.length > 0) {
    const item = queue.shift()!;
    let snap: Snapshot | null = null;
    try {
      snap = await item.run();
      if (snap) useEditor.getState().setSnap(snap);
    } catch (e) {
      useEditor.getState().toast({ kind: "error", text: errorText(e) });
    }
    item.waiters.forEach((w) => w(snap));
  }
  draining = false;
}

/** Resolves once every edit queued before it has been confirmed. */
export function whenIdle(): Promise<void> {
  return enqueue(async () => null).then(() => undefined);
}

let gesture = 0;
if (typeof window !== "undefined") {
  window.addEventListener("pointerdown", () => gesture++, true);
  window.addEventListener("focusin", () => gesture++, true);
}

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
  toastLift: 0,
  saveState: "saved",
  engineError: null,
  thumbs: {},
  filmstrips: {},
  waveforms: {},
  assetDrag: null,
  exportOpen: false,
  exportJobId: null,
  panelTab: "media",
  ratioOpen: false,
  keepTracks: {},

  setSnap: (snap, keepSelection = true) => {
    const ids = new Set(allClips(snap.project).map((c) => c.id));
    const kept = keepSelection ? get().selection.filter((id) => ids.has(id)) : [];
    const { cut, snap: prev } = get();
    // Only a new revision (or another project) is saved; a snapshot of an unchanged project, such
    // as after a failed import, gets no "saved" event and would leave Saving… forever.
    const changed = !prev || snap.path !== prev.path || snap.revision > prev.revision;
    set({
      snap,
      selection: snap.select.length > 0 ? snap.select : kept,
      cut: snap.select.length === 0 && keepSelection && cut && mainCuts(snap.project).some((c) => c.clipId === cut) ? cut : null,
      saveState: changed ? "saving" : get().saveState,
      timeUs: Math.min(get().timeUs, projectDuration(snap.project)),
    });
  },

  edit: (input, coalesce) => {
    // Keys are scoped to a gesture: one drag or one focus session is one undo step.
    const key = coalesce ? `${coalesce}#${gesture}` : null;
    return enqueue(async () => {
      const project = get().snap?.project;
      const cmd = typeof input === "function" ? (project ? input(project) : null) : input;
      if (!cmd || (Array.isArray(cmd) && cmd.length === 0)) return null;
      return Array.isArray(cmd) ? api.applyEdits(cmd, key ?? undefined) : api.applyEdit(cmd, key ?? undefined);
    }, key);
  },

  undo: async () => {
    await enqueue(async () => (get().snap?.canUndo ? api.undo() : null));
  },

  redo: async () => {
    await enqueue(async () => (get().snap?.canRedo ? api.redo() : null));
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

/**
 * Where the video ends: at the last picture or text, or at the last sound when there is no
 * picture. Music running past the picture is cut, like the engine's `Project::duration_us`.
 */
export function projectDuration(project: Project): number {
  const end = (audio: boolean) =>
    project.tracks.filter((t) => (t.kind === "audio") === audio).reduce((m, t) => t.clips.reduce((n, c) => Math.max(n, c.startUs + c.durationUs), m), 0);
  return end(false) || end(true);
}

/** The end of the last clip of any kind, so the timeline also shows music past the video's end. */
export function contentEnd(project: Project): number {
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

/** Queues an edit built from the clip's latest confirmed state; skipped when the clip is gone. */
export function editClip(clipId: string, build: (clip: Clip, track: Track, project: Project) => EditCmd | EditCmd[] | null, coalesce?: string) {
  return useEditor.getState().edit((project) => {
    const found = findClip(project, clipId);
    return found ? build(found.clip, found.track, project) : null;
  }, coalesce);
}

/** Queues one edit, all or nothing, built from the latest confirmed state of every clip in `ids`. */
export function editClips(ids: string[], build: (clip: Clip, track: Track, project: Project) => EditCmd | EditCmd[] | null, coalesce?: string) {
  return useEditor.getState().edit(
    (project) =>
      ids.flatMap((id) => {
        const found = findClip(project, id);
        const cmd = found ? build(found.clip, found.track, project) : null;
        return cmd === null ? [] : Array.isArray(cmd) ? cmd : [cmd];
      }),
    coalesce,
  );
}

/**
 * Writes a transform the way CapCut does: a clip with keyframes gets a keyframe at the
 * playhead, a clip without keyframes changes its single transform. `patch` is merged into the
 * transform at the playhead as it is when the edit is sent, so quick edits never undo each other.
 */
export function setClipTransform(clipId: string, patch: Partial<Transform>, coalesce?: string) {
  const timeUs = useEditor.getState().timeUs;
  return editClip(
    clipId,
    (clip, _track, project) => {
      const offset = clipOffset(clip, timeUs);
      const transform = { ...transformAt(clip, clip.content.transform, offset), ...patch };
      if (clip.keyframes.length === 0) return { type: "updateClip", clipId, transform };
      return { type: "setKeyframes", clipId, keyframes: upsertKeyframe(clip, offset, transform, keyframeTolerance(project.canvas.fps)) };
    },
    coalesce,
  );
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

/** The playhead is inside the clip with at least one frame left on each side. */
export function canSplitClip(clip: Clip, timeUs: number, fps: number) {
  const min = US / fps;
  return timeUs > clip.startUs + min && timeUs < clip.startUs + clip.durationUs - min;
}

/**
 * Selected clips under the playhead, or the main-track clip under it when no selected clip is.
 * A selected clip that the playhead only touches at its edge keeps the action on itself, so it
 * does nothing rather than cutting the main track instead.
 */
export function splitTargets(project: Project, selection: string[], timeUs: number): Clip[] {
  const touched = allClips(project).filter((c) => selection.includes(c.id) && timeUs >= c.startUs && timeUs <= c.startUs + c.durationUs);
  const under = (c: Clip) => canSplitClip(c, timeUs, project.canvas.fps);
  return (touched.length > 0 ? touched : mainClips(project)).filter(under);
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

/**
 * Whether ripple cuts (Q/W, transcript deletes) leave this track alone. By default music and
 * sound files stay in place, while sound detached from a video is cut with it so speech stays in sync.
 */
export function keepsInPlace(project: Project, track: Track, overrides: Record<string, boolean>): boolean {
  if (track.kind !== "audio") return false;
  if (track.id in overrides) return overrides[track.id];
  return track.clips.every((c) => {
    const m = c.content;
    return m.type === "media" && project.assets.find((a) => a.id === m.assetId)?.kind === "audio";
  });
}

/** Cuts `ranges` out of every track that does not stay in place and closes the gaps. */
export function rippleDelete(project: Project, ranges: TimeRange[]): EditCmd {
  const overrides = useEditor.getState().keepTracks;
  return {
    type: "rippleDeleteRanges",
    ranges: ranges.map((r) => ({ startUs: Math.round(r.startUs), endUs: Math.round(r.endUs) })),
    keepTrackIds: project.tracks.filter((t) => keepsInPlace(project, t, overrides)).map((t) => t.id),
  };
}

/**
 * CapCut's Q and W: delete the part of the clip left or right of the playhead. On the main track
 * the time is cut from every track that does not stay in place, so captions and overlays stay in
 * sync; clips on other tracks are trimmed. Selected clips under the playhead win over the main track.
 */
export async function deleteSide(side: "left" | "right") {
  const { snap, selection, timeUs, edit, seek, toast } = useEditor.getState();
  if (!snap) return;
  const t = Math.round(timeUs);
  const ids = splitTargets(snap.project, selection, t).map((c) => c.id);
  if (ids.length === 0) {
    toast({ kind: "info", text: "Move the playhead over a clip to delete one side of it." });
    return;
  }
  let cutAt: number | null = null;
  const done = await edit((project) => {
    const trims: EditCmd[] = [];
    const ranges: TimeRange[] = [];
    for (const id of ids) {
      const found = findClip(project, id);
      if (!found || !canSplitClip(found.clip, t, project.canvas.fps)) continue;
      const { clip, track } = found;
      const end = clip.startUs + clip.durationUs;
      const c = clip.content;
      if (track.id === MAIN_TRACK) {
        ranges.push(side === "left" ? { startUs: clip.startUs, endUs: t } : { startUs: t, endUs: end });
        if (side === "left") cutAt = Math.min(cutAt ?? clip.startUs, clip.startUs);
      } else if (side === "right") {
        trims.push({ type: "trimClip", clipId: id, startUs: clip.startUs, durationUs: t - clip.startUs, sourceInUs: null });
      } else {
        const sourceInUs = c.type === "media" ? c.sourceInUs + Math.round((t - clip.startUs) * c.speed) : null;
        trims.push({ type: "trimClip", clipId: id, startUs: t, durationUs: end - t, sourceInUs });
      }
    }
    // Trims first, at today's positions; the ripple then moves everything after the cut.
    return ranges.length > 0 ? [...trims, rippleDelete(project, ranges)] : trims;
  });
  // What was right of the playhead now starts where the deleted part began.
  if (done && cutAt !== null) seek(cutAt);
}

/** Selects every clip on the track of `clipId`. */
export function selectTrack(clipId: string) {
  const { snap, select } = useEditor.getState();
  const found = snap && findClip(snap.project, clipId);
  if (found) select(found.track.clips.map((c) => c.id));
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

/** Restyles every caption clip at once, as one undo step. */
export function applyCaptionStyle(style: TextStyle) {
  return useEditor.getState().edit((project) => project.tracks.find(isCaptionTrack)?.clips.map((c): EditCmd => ({ type: "updateClip", clipId: c.id, style })) ?? null);
}

/** Sets the font of every caption, keeping the rest of each caption's style, as one undo step. */
export function applyCaptionFont(fontFamily: string) {
  return useEditor.getState().edit(
    (project) =>
      project.tracks.find(isCaptionTrack)?.clips.flatMap((c): EditCmd[] => (c.content.type === "text" ? [{ type: "updateClip", clipId: c.id, style: { ...c.content.style, fontFamily } }] : [])) ?? null,
  );
}

export function openExport() {
  const { snap, exportJobId, jobs } = useEditor.getState();
  const running = exportJobId && jobs[exportJobId]?.status === "running";
  if (running || (snap && projectDuration(snap.project) > 0)) useEditor.setState({ exportOpen: true });
}
