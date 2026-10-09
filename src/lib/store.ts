import { create } from "zustand";
import { api, errorText, plainError } from "./api";
import { clipOffset, keyframeTolerance, transformAt, upsertKeyframe } from "./keyframes";
import { US } from "./time";
import type { Asset, Clip, EditCmd, Filmstrip, JobEvent, Project, ProjectVersion, Snapshot, TextStyle, TimeRange, Track, Transform, Transition, ZoomsApplied } from "./types";

export interface Toast {
  id: number;
  /** Errors and warnings stay until dismissed; info and success close by themselves unless `sticky`. */
  kind: "info" | "success" | "warning" | "error";
  text: string;
  sticky?: boolean;
  /** `valid`: the action is offered only while it holds, e.g. Undo while its step is still the newest. */
  action?: { label: string; run: () => void; valid?: () => boolean };
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
  /** Loaded blocks of each asset's waveform, by block index; every clip of the file draws from them. */
  waveforms: Record<string, Record<number, WaveBlock>>;
  assetDrag: AssetDrag | null;
  /** Where files dragged in from the desktop are over the window, in CSS pixels, while they include media. */
  fileDrag: { x: number; y: number } | null;
  /** Files being probed by an import, shown as placeholders until they arrive. */
  importing: { key: number; name: string; kind: Asset["kind"] }[];
  exportOpen: boolean;
  connectOpen: boolean;
  exportJobId: string | null;
  panelTab: PanelTab;
  ratioOpen: boolean;
  /** The home screen with every project, or the editor of the open project behind it. */
  view: "home" | "editor";
  /** The launcher over the editor: recent projects, new ones and search. */
  launcherOpen: boolean;

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
  /** Keeps blocks `first..=last` of an asset's waveform loaded until the returned function is called. */
  showWaveform: (assetId: string, first: number, last: number) => () => void;
  /** Loads the shown blocks of an asset again, e.g. once its sound is prepared or the id names another file. */
  reloadWaveform: (assetId: string) => void;
}

/** Waveform peaks per second of sound, and per block: the unit they are loaded and kept in (10.24 s). */
export const PEAKS_PER_SECOND = 50;
export const PEAKS_PER_BLOCK = 512;

export interface WaveBlock {
  peaks: Uint8Array;
  /** False while more of the block is still being decoded. */
  complete: boolean;
}

export type EditInput = EditCmd | EditCmd[] | ((project: Project) => EditCmd | EditCmd[] | null);

let toastId = 0;
const pending = new Set<string>();

/** Blocks kept that no waveform on screen shows: 256 are 128 KB, 44 minutes of sound. Those shown longest ago go first. */
const KEPT_BLOCKS = 256;
const BLOCKS_PER_REQUEST = 64;
/** The blocks each waveform on screen shows. */
const waveformViews = new Map<number, { assetId: string; first: number; last: number }>();
let waveformView = 0;
/** Loaded blocks as `assetId:index`, the one shown longest ago first. */
const waveformUse = new Set<string>();
/** Bumped when an asset's loaded blocks may be stale, so replies to requests sent before are dropped. */
const waveformGeneration = new Map<string, number>();
/** While a shown block is still being decoded it is asked for again, less often while nothing new comes. */
const waveformRetry = new Map<string, { timer: ReturnType<typeof setTimeout> | null; delay: number }>();

function waveformShown(assetId: string, block: number) {
  for (const v of waveformViews.values()) if (v.assetId === assetId && block >= v.first && block <= v.last) return true;
  return false;
}

function touchWaveform(key: string) {
  waveformUse.delete(key);
  waveformUse.add(key);
}

/** Drops loaded blocks past KEPT_BLOCKS that nothing shows, those shown longest ago first; the same object when none go. */
function evictWaveforms(waveforms: EditorState["waveforms"]) {
  if (waveformUse.size <= KEPT_BLOCKS) return waveforms;
  const out = { ...waveforms };
  const copied = new Set<string>();
  for (const key of waveformUse) {
    if (waveformUse.size <= KEPT_BLOCKS) break;
    const at = key.lastIndexOf(":");
    const [assetId, block] = [key.slice(0, at), Number(key.slice(at + 1))];
    if (waveformShown(assetId, block)) continue;
    waveformUse.delete(key);
    if (!out[assetId]) continue;
    if (!copied.has(assetId)) out[assetId] = { ...out[assetId] };
    copied.add(assetId);
    delete out[assetId][block];
  }
  return copied.size > 0 ? out : waveforms;
}

function retryWaveform(assetId: string, grew: boolean) {
  const retry = waveformRetry.get(assetId) ?? { timer: null, delay: 250 };
  retry.delay = grew ? 250 : Math.min(retry.delay * 2, 4000);
  if (!retry.timer)
    retry.timer = setTimeout(() => {
      retry.timer = null;
      loadWaveform(assetId);
    }, retry.delay);
  waveformRetry.set(assetId, retry);
}

/** Asks for the shown blocks of an asset that are not loaded or still growing, in runs of neighbouring blocks. */
function loadWaveform(assetId: string) {
  const epoch = useEditor.getState().snap?.sessionEpoch;
  const have = useEditor.getState().waveforms[assetId] ?? {};
  const generation = waveformGeneration.get(assetId) ?? 0;
  const key = (block: number) => `wave:${epoch}:${assetId}:${block}`;
  const wanted = new Set<number>();
  for (const v of waveformViews.values()) if (v.assetId === assetId) for (let b = v.first; b <= v.last; b++) wanted.add(b);
  const missing = [...wanted].filter((b) => !have[b]?.complete && !pending.has(key(b))).sort((a, b) => a - b);
  for (let i = 0; i < missing.length; ) {
    let j = i;
    while (j + 1 < missing.length && missing[j + 1] === missing[j] + 1 && j + 1 - i < BLOCKS_PER_REQUEST) j++;
    const [first, last] = [missing[i], missing[j]];
    i = j + 1;
    for (let b = first; b <= last; b++) pending.add(key(b));
    const current = () => useEditor.getState().snap?.sessionEpoch === epoch && (waveformGeneration.get(assetId) ?? 0) === generation;
    api
      .waveform(assetId, first * PEAKS_PER_BLOCK, (last + 1) * PEAKS_PER_BLOCK)
      .then((reply) => {
        if (!reply || !current()) return;
        const blocks = { ...useEditor.getState().waveforms[assetId] };
        let grew = false;
        for (let b = first; b <= last; b++) {
          const peaks = Uint8Array.from(reply.peaks.slice((b - first) * PEAKS_PER_BLOCK, (b - first + 1) * PEAKS_PER_BLOCK));
          grew ||= peaks.length > (blocks[b]?.peaks.length ?? 0);
          // A full block no longer changes, even while the rest of the file decodes.
          blocks[b] = { peaks, complete: reply.complete || peaks.length === PEAKS_PER_BLOCK };
          touchWaveform(`${assetId}:${b}`);
        }
        useEditor.setState({ waveforms: evictWaveforms({ ...useEditor.getState().waveforms, [assetId]: blocks }) });
        if (!reply.complete) retryWaveform(assetId, grew);
      })
      .catch(() => {})
      .finally(() => {
        for (let b = first; b <= last; b++) pending.delete(key(b));
        // Asked for before the blocks went stale: ask again.
        if (useEditor.getState().snap?.sessionEpoch === epoch && !current()) loadWaveform(assetId);
      });
  }
}

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
      if (text.startsWith("RUN_ACTIVE")) noticeAiRun();
      else if (!text.startsWith("EPOCH_CHANGED")) useEditor.getState().toast({ kind: "error", text });
    }
    item.waiters.forEach((w) => w(snap));
  }
  draining = false;
}

/**
 * Undo for a toast about the step that produced `snap`. It undoes only while that step is still
 * the newest one, so a later edit is never undone in its place, and the toast hides it once the
 * history moved on.
 */
export function undoAction(snap: Snapshot): NonNullable<Toast["action"]> {
  return { label: "Undo", valid: () => isNewest(snap), run: () => void undoStep(snap) };
}

function isNewest({ revision, sessionEpoch }: Snapshot) {
  const now = useEditor.getState().snap;
  return !!now && now.revision === revision && now.sessionEpoch === sessionEpoch && now.canUndo;
}

/** Undoes the step that produced `snap` if it is still the newest; resolves whether it did. */
export function undoStep(snap: Snapshot): Promise<boolean> {
  if (aiLocked()) return Promise.resolve(false);
  // Checked again when its turn in the queue comes, after any edit made before it.
  return enqueue(async (epoch) => (isNewest(snap) ? api.undo(epoch) : null)).then((done) => !!done);
}

/** Restores a kept version as one undo step, after the edits queued before it; a toast offers Undo. */
export async function restoreVersion(version: ProjectVersion) {
  if (aiLocked()) return;
  const snap = await enqueue((epoch) => api.restoreVersion(version.index, epoch));
  if (snap) useEditor.getState().toast({ kind: "info", text: `Restored “${version.label}”`, action: undoAction(snap) });
}

/** Why editing is locked while an agent's run is open. */
export const AI_EDITING = "AI is editing. Stop it to edit yourself.";

/**
 * Says that the AI is editing, with a way to take over. It stays until the run ends; dismissed,
 * it comes back with the next refused edit, so an edit never goes unanswered.
 */
function noticeAiRun() {
  if (useEditor.getState().toasts.some((t) => t.text === AI_EDITING)) return;
  useEditor.getState().toast({ kind: "info", sticky: true, text: AI_EDITING, action: { label: "Stop and edit", run: () => void stopAiRun(), valid: () => !!useEditor.getState().aiRun } });
}

/** True while an agent's run is open: the edit is not sent, and the notice shows once. */
export function aiLocked(): boolean {
  if (!useEditor.getState().aiRun) return false;
  noticeAiRun();
  return true;
}

let discarding = false;

/**
 * Ends the agent's run so the user can edit: kept, Undo then removes the whole run; with `discard`,
 * its changes are taken back. False when it could not be stopped.
 */
export async function stopAiRun(discard = false) {
  // Kept until the run ends: one whose taking back could not be saved is still taken back when
  // it ends later, also by Stop and edit.
  discarding ||= discard;
  try {
    useEditor.getState().setSnap(await api.stopRun(currentEpoch(), discard));
    return true;
  } catch (e) {
    useEditor.getState().toast({ kind: "error", text: errorText(e) });
    return false;
  }
}

/**
 * Whether the user took back the run that just ended, asked once per run. Its end then offers no
 * Undo: the run is no longer in the history, so Undo would take back the change before it.
 */
export function runWasDiscarded() {
  const discarded = discarding;
  discarding = false;
  return discarded;
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

/**
 * Media previews arrive as PNG data URLs of up to a few MB. As a blob URL the same image is a short string,
 * so a `src` or background holding it costs nothing to compare when a clip or tile re-renders.
 */
export function blobUrl(url: string): string {
  const head = /^data:([^;,]+);base64,/.exec(url);
  if (!head) return url;
  const bin = atob(url.slice(head[0].length));
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return URL.createObjectURL(new Blob([bytes], { type: head[1] }));
}

/** Frees the blob URLs of previews dropped from the caches. */
function revokePreviews(thumbs: (string | null)[], strips: (Filmstrip | null)[]) {
  for (const url of [...thumbs, ...strips.map((f) => f?.url ?? null)]) if (url?.startsWith("blob:")) URL.revokeObjectURL(url);
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
  fileDrag: null,
  importing: [],
  exportOpen: false,
  connectOpen: false,
  exportJobId: null,
  panelTab: "media",
  ratioOpen: false,
  view: "home",
  launcherOpen: false,

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
    // An asset id that now names another file, e.g. media an agent replaced, gets new previews.
    const rebound = switched ? [] : prev.project.assets.filter((a) => snap.project.assets.some((b) => b.id === a.id && b.path !== a.path)).map((a) => a.id);
    const without = <T,>(cache: Record<string, T>) => Object.fromEntries(Object.entries(cache).filter(([id]) => !rebound.includes(id)));
    const dropped = (id: string) => switched || rebound.includes(id);
    revokePreviews(
      Object.entries(get().thumbs).flatMap(([id, url]) => (dropped(id) ? [url] : [])),
      Object.entries(get().filmstrips).flatMap(([id, strip]) => (dropped(id) ? [strip] : [])),
    );
    set({
      snap,
      selection: snap.select.length > 0 ? snap.select : kept,
      cut: snap.select.length === 0 && keepSelection && cut && mainCuts(snap.project).some((c) => c.clipId === cut) ? cut : null,
      saveState: switched ? "saved" : changed ? "saving" : get().saveState,
      timeUs: Math.min(get().timeUs, projectDuration(snap.project)),
      aiRun: snap.openRunLabel,
      // A copied project can reuse asset ids for other files, so media previews start over.
      ...(switched ? { thumbs: {}, filmstrips: {}, waveforms: {} } : {}),
      ...(rebound.length > 0 ? { thumbs: without(get().thumbs), filmstrips: without(get().filmstrips), waveforms: without(get().waveforms) } : {}),
    });
    if (switched) waveformUse.clear();
    for (const id of rebound) get().reloadWaveform(id);
    if (switched && snap.agentBridgeError) {
      get().toast({ kind: "info", text: "AI agents can't connect to this project while it is open here. Your own editing works normally." });
    }
  },

  edit: (input, coalesce) => {
    if (aiLocked()) return Promise.resolve(null);
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
    if (!aiLocked()) await enqueue(async (epoch) => (get().snap?.canUndo ? api.undo(epoch) : null));
  },

  redo: async () => {
    if (!aiLocked()) await enqueue(async (epoch) => (get().snap?.canRedo ? api.redo(epoch) : null));
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
    if (t.kind === "error") t = { ...t, text: plainError(t.text) };
    // A toast whose action no longer applies, or one saying the same again, makes room.
    let kept = get().toasts.filter((o) => (o.action?.valid?.() ?? true) && !(o.kind === t.kind && o.text === t.text && !o.action));
    // Four at most. Routine news goes first, so errors and warnings stay until they are dismissed.
    while (kept.length >= 4) {
      const routine = kept.findIndex((o) => (o.kind === "info" || o.kind === "success") && !o.sticky);
      kept = kept.filter((_, i) => i !== Math.max(0, routine));
    }
    // The toast times itself out (Toasts.tsx), so hovering or focusing it can pause the clock.
    set({ toasts: [...kept, { ...t, id }] });
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
      .then((url) => keep(url && blobUrl(url)), () => keep(null))
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
      .then((strip) => keep(strip && { ...strip, url: blobUrl(strip.url) }), () => keep(null))
      .finally(() => pending.delete(key));
  },

  showWaveform: (assetId, first, last) => {
    const view = ++waveformView;
    waveformViews.set(view, { assetId, first, last });
    for (let b = first; b <= last; b++) if (waveformUse.has(`${assetId}:${b}`)) touchWaveform(`${assetId}:${b}`);
    loadWaveform(assetId);
    return () => {
      waveformViews.delete(view);
      // After the commit, so a waveform that only moved has shown its new blocks first.
      queueMicrotask(() => {
        const waveforms = get().waveforms;
        const kept = evictWaveforms(waveforms);
        if (kept !== waveforms) set({ waveforms: kept });
      });
    };
  },

  reloadWaveform: (assetId) => {
    waveformGeneration.set(assetId, (waveformGeneration.get(assetId) ?? 0) + 1);
    const blocks = get().waveforms[assetId];
    // Kept on screen until the new ones arrive.
    if (blocks) set({ waveforms: { ...get().waveforms, [assetId]: Object.fromEntries(Object.entries(blocks).map(([b, block]) => [b, { ...block, complete: false }])) } });
    for (const key of waveformUse) if (key.startsWith(`${assetId}:`) && !blocks?.[Number(key.slice(assetId.length + 1))]) waveformUse.delete(key);
    loadWaveform(assetId);
  },
}));

// When the run ends its notice goes.
useEditor.subscribe((s, prev) => {
  if (!prev.aiRun || s.aiRun) return;
  if (s.toasts.some((t) => t.text === AI_EDITING)) useEditor.setState({ toasts: s.toasts.filter((t) => t.text !== AI_EDITING) });
});

/** While an agent's run is open the editing controls are locked. */
export const useAiLocked = () => useEditor((s) => s.aiRun !== null);

/**
 * The playhead for readouts that follow it, such as keyframed values in the inspector: in 0.1 s steps while
 * playing, so they re-render ten times a second instead of every frame; exact when paused or scrubbing.
 * Edits read the exact `timeUs`.
 */
export const readoutTime = (s: { playing: boolean; timeUs: number }) => (s.playing ? Math.floor(s.timeUs / 100_000) * 100_000 : s.timeUs);

/**
 * Caches a value derived from a project. Snapshots are never changed in place, so a new value means a new
 * project object. Selectors run on every playback frame (the store holds the playhead), so what they derive
 * from the whole project must not be rebuilt each time.
 */
function perProject<T>(derive: (project: Project) => T): (project: Project) => T {
  const cache = new WeakMap<Project, T>();
  return (project) => {
    if (!cache.has(project)) cache.set(project, derive(project));
    return cache.get(project) as T;
  };
}

export const allClips: (project: Project) => Clip[] = perProject((project) => project.tracks.flatMap((t) => t.clips));

/**
 * Where the video ends: at the last picture or text, or at the last sound when there is no
 * picture. Music running past the picture is cut, like the engine's `Project::duration_us`.
 */
export const projectDuration: (project: Project) => number = perProject((project) => {
  const end = (audio: boolean) =>
    project.tracks.filter((t) => (t.kind === "audio") === audio).reduce((m, t) => t.clips.reduce((n, c) => Math.max(n, c.startUs + c.durationUs), m), 0);
  return end(false) || end(true);
});

/** The end of the last clip of any kind, so the timeline also shows music past the video's end. */
export const contentEnd: (project: Project) => number = perProject((project) => allClips(project).reduce((m, c) => Math.max(m, c.startUs + c.durationUs), 0));

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
export const mainCuts: (project: Project) => { clipId: string; atUs: number; transition: Transition | null }[] = perProject((project) =>
  mainClips(project)
    .slice(1)
    .map((c) => ({ clipId: c.id, atUs: c.startUs, transition: c.transitionIn })),
);

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
type TransformPatch = Partial<Transform> | ((current: Transform) => Partial<Transform>);

/** The edit that changes the transform at `timeUs`: the clip's own, or once it has keyframes, the keyframe there. */
export function transformEdit(clip: Clip, project: Project, timeUs: number, patch: TransformPatch): EditCmd {
  const offset = clipOffset(clip, timeUs);
  const current = transformAt(clip, clip.content.transform, offset);
  const transform = { ...current, ...(typeof patch === "function" ? patch(current) : patch) };
  if (clip.keyframes.length === 0) return { type: "updateClip", clipId: clip.id, transform };
  return { type: "setKeyframes", clipId: clip.id, keyframes: upsertKeyframe(clip, offset, transform, keyframeTolerance(project.canvas.fps)) };
}

export function setClipTransform(clipId: string, patch: TransformPatch, coalesce?: string) {
  const timeUs = useEditor.getState().timeUs;
  return editClip(clipId, (clip, _track, project) => transformEdit(clip, project, timeUs, patch), coalesce);
}

export async function deleteSelection() {
  const { selection, cut, edit, select, selectCut, toast } = useEditor.getState();
  if (cut) {
    const snap = await edit({ type: "setTransition", clipId: cut, transition: null });
    if (snap) {
      selectCut(null);
      toast({ kind: "info", text: "Transition removed", action: undoAction(snap) });
    }
    return;
  }
  if (selection.length === 0) return;
  const snap = await edit({ type: "deleteClips", clipIds: selection });
  if (snap) {
    // Drops only the deleted clips: the timeline may already have selected the clip that took focus.
    select(useEditor.getState().selection.filter((id) => !selection.includes(id)));
    toast({ kind: "info", text: selection.length === 1 ? "Clip deleted" : `${selection.length} clips deleted`, action: undoAction(snap) });
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

/** Moves the sound of each clip to an audio track, as one undo step. */
export async function detachAudio(clipIds: string[]) {
  const { edit, toast } = useEditor.getState();
  const snap = await edit(clipIds.map((clipId): EditCmd => ({ type: "detachAudio", clipId })));
  const text = clipIds.length === 1 ? "Audio moved to its own track" : `Audio of ${clipIds.length} clips detached`;
  if (snap) toast({ kind: "info", text, action: undoAction(snap) });
}

/** Turns Clean voice on or off for the media clips in `ids`, as one undo step. */
export function setCleanVoice(ids: string[], on: boolean) {
  return editClips(ids, (c) => (c.content.type === "media" ? { type: "updateClip", clipId: c.id, cleanVoice: on } : null));
}

/** How far the cleaned voice of these files is prepared (0 to 1) while that runs, else null. */
export function useVoicePreparation(assetIds: string[]) {
  return useEditor((s) => {
    const running = assetIds.map((id) => s.jobs[`voice:${id}`]).filter((j) => j?.status === "running");
    return running.length > 0 ? Math.min(...running.map((j) => j.progress)) : null;
  });
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

/**
 * Punches in on the suggested sentences after the edits queued before, as one undo step, and offers
 * Undo. Clips with keyframes keep their motion and the toast says so. Resolves to whether it ran.
 */
export async function applyZooms(key: string, zooms: { from: number; to: number; scale: number }[]): Promise<boolean> {
  if (aiLocked()) return false;
  const out: { done?: ZoomsApplied } = {};
  await enqueue(async (epoch) => (out.done = await api.applyZooms(key, zooms, epoch)).snapshot);
  const done = out.done;
  if (!done) return false;
  const clips = (n: number) => `${n} clip${n === 1 ? "" : "s"} with keyframes`;
  if (!done.changed) {
    useEditor.getState().toast({ kind: "info", text: `Nothing zoomed: the sentences lie on ${clips(done.skipped)}, which keep their own motion.` });
    return true;
  }
  const kept = done.skipped > 0 ? `; ${clips(done.skipped)} kept their own motion` : "";
  const text = `Zoomed in on ${zooms.length} sentence${zooms.length === 1 ? "" : "s"}${kept}`;
  useEditor.getState().toast({ kind: "info", text, action: undoAction(done.snapshot) });
  return true;
}

export function openExport() {
  const { snap, exportJobId, jobs } = useEditor.getState();
  const running = exportJobId && jobs[exportJobId]?.status === "running";
  if (running || (snap && projectDuration(snap.project) > 0)) useEditor.setState({ exportOpen: true });
}
