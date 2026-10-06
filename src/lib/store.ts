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
  /** Label of the AI run editing the project right now, or null. */
  aiRun: string | null;
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

  /** `opened`: the snapshot of a project just opened, created or booted; others must belong to the open session. */
  setSnap: (snap: Snapshot, keepSelection?: boolean, opened?: boolean) => void;
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
  run: (epoch: string | undefined) => Promise<Snapshot | null>;
  key: string | null;
  /** The session open when the change was made; the backend refuses it once another is open. */
  epoch: string | undefined;
  waiters: ((snap: Snapshot | null) => void)[];
}
const queue: QueuedEdit[] = [];
let draining = false;

/** The open project's session, for commands that change it. */
export const currentEpoch = () => useEditor.getState().snap?.sessionEpoch;

export function enqueue(run: (epoch: string | undefined) => Promise<Snapshot | null>, key: string | null = null): Promise<Snapshot | null> {
  return new Promise((resolve) => {
    const epoch = currentEpoch();
    const last = queue[queue.length - 1];
    if (key && last?.key === key && last.epoch === epoch) {
      last.run = run;
      last.waiters.push(resolve);
    } else queue.push({ run, key, epoch, waiters: [resolve] });
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
      snap = await item.run(item.epoch);
      if (snap) useEditor.getState().setSnap(snap);
    } catch (e) {
      const text = errorText(e);
      // EPOCH_CHANGED: the change was made for the project open before, which the editor no longer shows.
      if (text.startsWith("RUN_ACTIVE")) useEditor.getState().toast({ kind: "info", text: "AI is editing. Stop it to edit yourself.", action: { label: "Stop and edit", run: stopAiRun } });
      else if (!text.startsWith("EPOCH_CHANGED")) useEditor.getState().toast({ kind: "error", text });
    }
    item.waiters.forEach((w) => w(snap));
  }
  draining = false;
}

/** Ends the agent's run with its changes kept, so the user can edit; Undo then removes the whole run. */
export async function stopAiRun() {
  try {
    useEditor.getState().setSnap(await api.stopRun(currentEpoch()));
  } catch (e) {
    useEditor.getState().toast({ kind: "error", text: errorText(e) });
  }
}

/**
 * Opens another project once the changes queued for this one are done, so none of them lands in
 * the new project; changes made while it opens are refused by the backend.
 */
export async function switchProject(open: () => Promise<Snapshot>) {
  await whenIdle();
  useEditor.getState().setSnap(await open(), false, true);
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
  aiRun: null,
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

  setSnap: (snap, keepSelection = true, opened = false) => {
    const previous = get().snap;
    if (previous?.sessionEpoch === snap.sessionEpoch && snap.revision < previous.revision) return;
    // A late reply from the project open before must not switch the editor back to it.
    if (previous && previous.sessionEpoch !== snap.sessionEpoch && !opened) return;
    const ids = new Set(allClips(snap.project).map((c) => c.id));
    const kept = keepSelection ? get().selection.filter((id) => ids.has(id)) : [];
    const { cut, snap: prev } = get();
    // Opening loads a durable project. Only a new revision waits for a saved event.
    const switched = !prev || snap.sessionEpoch !== prev.sessionEpoch;
    const changed = !switched && snap.revision > prev.revision;
    set({
      snap,
      selection: snap.select.length > 0 ? snap.select : kept,
      cut: snap.select.length === 0 && keepSelection && cut && mainCuts(snap.project).some((c) => c.clipId === cut) ? cut : null,
      saveState: switched ? "saved" : changed ? "saving" : get().saveState,
      timeUs: Math.min(get().timeUs, projectDuration(snap.project)),
      aiRun: snap.openRunLabel,
      // A copied project can reuse asset ids for other files, so media previews start over.
      ...(switched ? { thumbs: {}, filmstrips: {}, waveforms: {} } : {}),
    });
  },

  edit: (input, coalesce) => {
    // Keys are scoped to a gesture: one drag or one focus session is one undo step.
    const key = coalesce ? `${coalesce}#${gesture}` : null;
    return enqueue(async (epoch) => {
      const project = get().snap?.project;
      const cmd = typeof input === "function" ? (project ? input(project) : null) : input;
      if (!cmd || (Array.isArray(cmd) && cmd.length === 0)) return null;
      return Array.isArray(cmd) ? api.applyEdits(cmd, key, epoch) : api.applyEdit(cmd, key, epoch);
    }, key);
  },

  undo: async () => {
    await enqueue(async (epoch) => (get().snap?.canUndo ? api.undo(epoch) : null));
  },

  redo: async () => {
    await enqueue(async (epoch) => (get().snap?.canRedo ? api.redo(epoch) : null));
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

  // Loads are per project: a reply that arrives after another project opened is dropped.
  loadThumb: (assetId) => {
    const epoch = get().snap?.sessionEpoch;
    const key = `thumb:${epoch}:${assetId}`;
    if (assetId in get().thumbs || pending.has(key)) return;
    pending.add(key);
    const keep = (url: string | null) => get().snap?.sessionEpoch === epoch && set({ thumbs: { ...get().thumbs, [assetId]: url } });
    api
      .thumbnail(assetId)
      .then(keep, () => keep(null))
      .finally(() => pending.delete(key));
  },

  loadFilmstrip: (assetId) => {
    const epoch = get().snap?.sessionEpoch;
    const key = `strip:${epoch}:${assetId}`;
    if (assetId in get().filmstrips || pending.has(key)) return;
    pending.add(key);
    const keep = (strip: Filmstrip | null) => get().snap?.sessionEpoch === epoch && set({ filmstrips: { ...get().filmstrips, [assetId]: strip } });
    api
      .filmstrip(assetId)
      .then(keep, () => keep(null))
      .finally(() => pending.delete(key));
  },

  loadWaveform: (assetId, force = false) => {
    const epoch = get().snap?.sessionEpoch;
    const key = `wave:${epoch}:${assetId}`;
    if ((!force && get().waveforms[assetId]) || pending.has(key)) return;
    pending.add(key);
    api
      .waveform(assetId)
      .then((peaks) => peaks && get().snap?.sessionEpoch === epoch && set({ waveforms: { ...get().waveforms, [assetId]: peaks } }))
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
  const at = Math.round(timeUs);
  const ids = splitTargets(snap.project, selection, at).map((c) => c.id);
  if (ids.length === 0) {
    toast({ kind: "info", text: "Move the playhead over a clip to split it." });
    return;
  }
  // One batch: one undo step, and every second half stays selected. An earlier queued edit may
  // have removed a target; it is skipped rather than replaced by another clip.
  await edit((project) =>
    ids.flatMap((id): EditCmd[] => {
      const found = findClip(project, id);
      return found && canSplitClip(found.clip, at, project.canvas.fps) ? [{ type: "splitClip", clipId: id, atUs: at }] : [];
    }),
  );
}

export async function duplicateSelection() {
  const { selection, edit } = useEditor.getState();
  if (selection.length === 0) return;
  // One batch: one undo step, and the snapshot selects every copy.
  await edit(selection.map((id): EditCmd => ({ type: "duplicateClip", clipId: id })));
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
    return ranges.length > 0 ? [...trims, { type: "rippleDeleteRanges", ranges }] : trims;
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

/** Restyles every caption clip at once, as one undo step, keeping each caption's wrap width (the safe area). */
export function applyCaptionStyle(style: TextStyle) {
  return useEditor.getState().edit(
    (project) =>
      project.tracks.find(isCaptionTrack)?.clips.flatMap((c): EditCmd[] => (c.content.type === "text" ? [{ type: "updateClip", clipId: c.id, style: { ...style, maxWidth: c.content.style.maxWidth } }] : [])) ?? null,
  );
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
