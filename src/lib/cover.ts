import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { create } from "zustand";
import presets from "../../assets/presets/thumbnails.json";
import { api, errorText, plainError } from "./api";
import { DEFAULT_TRANSFORM } from "./presets";
import { aiLocked, currentEpoch, projectDuration, undoAction, useEditor } from "./store";
import { US } from "./time";
import type { CoverCandidate, CoverPick, JobEvent, Project, TextStyle, Thumbnail, ThumbnailFormat, ThumbnailText, VisionModels } from "./types";

export const COVER_FORMATS: { id: ThumbnailFormat; label: string; short: string; noun: string; width: number; height: number }[] = [
  { id: "cover_9x16", label: "9:16 Cover", short: "9:16", noun: "cover", width: 1080, height: 1920 },
  { id: "youtube_16x9", label: "16:9 YouTube", short: "16:9", noun: "YouTube thumbnail", width: 1280, height: 720 },
];

export const coverFormat = (id: ThumbnailFormat) => COVER_FORMATS.find((f) => f.id === id)!;

/** Text looks for covers, shared with the engine's agents (assets/presets/thumbnails.json). Unlike text presets they carry their font. */
export const COVER_PRESETS: { name: string; behind: boolean; style: TextStyle }[] = presets;

interface CoverState {
  /** The cover editor shows this format in place of the video; null shows the video. */
  open: ThumbnailFormat | null;
  /** What is selected on the open cover: a text by its index, or the frame. */
  selected: number | "frame" | null;
  /** The playhead and clip selection from before the cover editor opened, given back when it closes. */
  before: { timeUs: number; selection: string[] } | null;
  /** What Pick for me found, per format, in the session it was picked in. */
  picks: Partial<Record<ThumbnailFormat, { epoch: string; candidates: CoverCandidate[] }>>;
  /** The jobs this editor started: picking frames, masking the person, the model download. */
  pickJob: string | null;
  maskJob: string | null;
  modelsJob: string | null;
  /** The cover export the Export dialog started; with the dialog closed its end is a toast. */
  exportJob: string | null;
  /** Why the last pick or mask failed, until another one starts. */
  failed: string | null;
  /** The last failure was a cover model that could not be loaded: downloading them again repairs it. */
  damaged: boolean;
  /** The cover's frame when Pick for me started; a frame chosen meanwhile is kept. */
  pickFrom: { format: ThumbnailFormat; timeUs: number | null } | null;
  /** Pick for me kept the frame chosen while it ran, for this format. */
  kept: ThumbnailFormat | null;
  /** Counts finished masks, so the canvas looks for the new mask in the cache. */
  masks: number;
  /** The share of each text of the cover on screen that the person covers; empty until the person is cut out. */
  hidden: number[];
  models: VisionModels | null;
  safeZones: boolean;
}

const SAFE_ZONES_KEY = "nuzky.coverSafeZones";

export const useCover = create<CoverState>(() => ({
  open: null,
  selected: null,
  before: null,
  picks: {},
  pickJob: null,
  maskJob: null,
  modelsJob: null,
  exportJob: null,
  failed: null,
  damaged: false,
  pickFrom: null,
  kept: null,
  masks: 0,
  hidden: [],
  models: null,
  safeZones: localStorage.getItem(SAFE_ZONES_KEY) !== "0",
}));

export function thumbnailOf(project: Project | undefined, format: ThumbnailFormat): Thumbnail | undefined {
  return project?.thumbnails?.find((t) => t.format === format);
}

export function setSafeZones(on: boolean) {
  localStorage.setItem(SAFE_ZONES_KEY, on ? "1" : "0");
  useCover.setState({ safeZones: on });
}

/** Asks again whether the cover models can run here and what is left to download. */
export async function loadModels() {
  try {
    useCover.setState({ models: await api.visionModels() });
  } catch {
    useCover.setState({ models: null });
  }
}

/**
 * Opens the cover editor on `format`, else on the 9:16 cover, or the YouTube one when only it exists. The
 * playhead goes to the cover's frame: in the cover editor the playhead is the frame.
 */
export function openCover(format?: ThumbnailFormat) {
  const { snap, playing, timeUs, selection } = useEditor.getState();
  if (!snap || projectDuration(snap.project) <= 0) return;
  const chosen = format ?? (thumbnailOf(snap.project, "cover_9x16") || !thumbnailOf(snap.project, "youtube_16x9") ? "cover_9x16" : "youtube_16x9");
  if (playing) void api.pause();
  const before = useCover.getState().before ?? { timeUs, selection };
  useCover.setState({ open: chosen, selected: null, before });
  useEditor.getState().select([]);
  showFrame(chosen);
  void loadModels();
}

/** Shows another format in the cover editor; the playhead goes to its frame. */
export function switchFormat(format: ThumbnailFormat) {
  useCover.setState({ open: format, selected: null });
  showFrame(format);
}

/** Leaves the cover editor; the playhead and the clip selection come back as they were. */
export function closeCover() {
  const before = useCover.getState().before;
  useCover.setState({ open: null, selected: null, before: null });
  void api.coverClose();
  if (before) {
    useEditor.getState().seek(before.timeUs);
    useEditor.getState().select(before.selection);
  }
}

/** The playhead time the cover editor set itself, which is not the user choosing a frame. */
let synced: number | null = null;

/** Moves the playhead to the frame of the cover of `format`, when it has one. */
function showFrame(format: ThumbnailFormat) {
  const cover = thumbnailOf(useEditor.getState().snap?.project, format);
  if (!cover) return;
  useEditor.getState().seek(cover.timeUs);
  synced = useEditor.getState().timeUs;
}

/** The last frame time a cover can have: the frame shown when the playhead rests on the end. */
const lastFrame = (project: Project) => Math.max(0, projectDuration(project) - Math.ceil(US / project.canvas.fps));

/** Frame changes the playhead asked for that are not confirmed yet; their snapshots must not move it back. */
let choosing = 0;

// In the cover editor the playhead is the cover's frame. Moving it chooses the frame, one undo step per gesture;
// a frame changed by undo, a candidate or an agent moves the playhead.
useEditor.subscribe((s, prev) => {
  const format = useCover.getState().open;
  if (!format || !s.snap) return;
  // Another project opened: its editor starts on the video, and what was picked belongs to the one before.
  if (prev.snap && s.snap.sessionEpoch !== prev.snap.sessionEpoch) {
    useCover.setState({ open: null, selected: null, before: null });
    return;
  }
  const cover = thumbnailOf(s.snap.project, format);
  const before = thumbnailOf(prev.snap?.project, format);
  if (cover && cover.timeUs !== before?.timeUs && choosing === 0) {
    if (Math.abs(cover.timeUs - s.timeUs) > 1) showFrame(format);
    return;
  }
  if (s.timeUs === prev.timeUs || s.timeUs === synced) return;
  synced = null;
  if (!cover) return;
  if (s.aiRun) {
    // The frame stays the agent's; the playhead goes back to it and the lock says why.
    aiLocked();
    showFrame(format);
    return;
  }
  choosing++;
  void chooseFrame(format, Math.min(s.timeUs, lastFrame(s.snap.project)), "frame").finally(() => choosing--);
});

/**
 * Queues a change of the cover of `format`, built from the latest confirmed project like any edit, as one
 * undo step per gesture or focus session. `build` gets the cover as it is (undefined when there is none)
 * and returns the new one, or null to change nothing.
 */
export function editCover(format: ThumbnailFormat, build: (current: Thumbnail | undefined, project: Project) => Thumbnail | null, coalesce?: string) {
  return useEditor.getState().edit((project) => {
    const next = build(thumbnailOf(project, format), project);
    return next ? { type: "setThumbnail", thumbnail: next } : null;
  }, coalesce && `cover:${format}:${coalesce}`);
}

const isVertical = (project: Project) => project.canvas.height > project.canvas.width;

/**
 * A new cover from the frame at `timeUs`: the frame fills the thumbnail as it would on the canvas. A YouTube
 * thumbnail of a vertical video gets the frame blurred and dimmed behind the person, as agents make it, when
 * the person can be cut out on this computer.
 */
export function newCover(format: ThumbnailFormat, project: Project, timeUs: number): Thumbnail {
  const sides = format === "youtube_16x9" && isVertical(project) && !useCover.getState().models?.unavailable;
  return {
    format,
    timeUs: Math.round(timeUs),
    frame: DEFAULT_TRANSFORM,
    background: { picture: true, blur: sides ? 1 : 0, dim: sides ? 0.2 : 0, color: "#000000" },
    texts: [],
    outline: null,
  };
}

/** Sets the cover's frame, making the cover when there is none, as one undo step. */
export function chooseFrame(format: ThumbnailFormat, timeUs: number, coalesce?: string) {
  return editCover(
    format,
    (current, project) => {
      const t = Math.round(Math.min(timeUs, lastFrame(project)));
      return current ? (current.timeUs === t ? null : { ...current, timeUs: t }) : newCover(format, project, t);
    },
    coalesce,
  );
}

/** A text in the look of `preset`, sized for the format, in the upper part where the apps leave it free. */
export function presetText(format: ThumbnailFormat, preset: (typeof COVER_PRESETS)[number], text = "YOUR HOOK"): ThumbnailText {
  // The presets are sized for the 1080 px wide cover; a YouTube thumbnail is seen smaller, about two thirds.
  const k = format === "youtube_16x9" ? 2 / 3 : 1;
  return {
    text,
    style: { ...preset.style, fontSize: Math.round(preset.style.fontSize * k), strokeWidth: preset.style.strokeWidth * k },
    transform: { ...DEFAULT_TRANSFORM, y: format === "youtube_16x9" ? -0.2 : -0.22 },
    behind: preset.behind,
  };
}

/** Adds a text in the look of `preset` and selects it, as one undo step; without a cover yet, it makes one at the playhead. */
export async function addText(format: ThumbnailFormat, preset: (typeof COVER_PRESETS)[number]) {
  const timeUs = useEditor.getState().timeUs;
  const snap = await editCover(format, (current, project) => {
    const cover = current ?? newCover(format, project, Math.min(timeUs, lastFrame(project)));
    return { ...cover, texts: [...cover.texts, presetText(format, preset)] };
  });
  const count = thumbnailOf(snap?.project, format)?.texts.length ?? 0;
  if (snap && count > 0) useCover.setState({ selected: count - 1 });
  return snap;
}

/** Changes one text of the cover, built from its latest confirmed state. */
export function editText(format: ThumbnailFormat, index: number, change: (text: ThumbnailText) => ThumbnailText | null, coalesce?: string) {
  return editCover(
    format,
    (current) => {
      const text = current?.texts[index];
      const next = text && change(text);
      return current && next ? { ...current, texts: current.texts.map((t, i) => (i === index ? next : t)) } : null;
    },
    coalesce && `text:${index}:${coalesce}`,
  );
}

/** Removes a text of the cover, with Undo in a toast. */
export async function deleteText(format: ThumbnailFormat, index: number) {
  const snap = await editCover(format, (c) => (c && c.texts[index] ? { ...c, texts: c.texts.filter((_, i) => i !== index) } : null));
  if (!snap) return;
  useCover.setState({ selected: null });
  useEditor.getState().toast({ kind: "info", text: "Text removed from the cover", action: undoAction(snap) });
}

/** Starts Pick for me for `format`. The models download on first use inside the same job. */
export async function pick(format: ThumbnailFormat) {
  if (useEditor.getState().aiRun) return;
  const timeUs = thumbnailOf(useEditor.getState().snap?.project, format)?.timeUs ?? null;
  useCover.setState({ failed: null, kept: null, pickFrom: { format, timeUs } });
  try {
    useCover.setState({ pickJob: await api.startCoverPick(format, currentEpoch()) });
  } catch (e) {
    useCover.setState({ failed: plainError(errorText(e)) });
  }
}

/** Starts finding the person in the frame of the cover, once per frame, when the cover cuts them out. */
export async function mask(timeUs: number) {
  const { maskJob } = useCover.getState();
  if (maskJob && useEditor.getState().jobs[maskJob]?.status === "running") return;
  useCover.setState({ failed: null });
  try {
    useCover.setState({ maskJob: await api.startCoverMask(timeUs, currentEpoch()) });
  } catch (e) {
    useCover.setState({ failed: plainError(errorText(e)) });
  }
}

/** Downloads the cover models without picking, e.g. before using Behind person; after a damaged model, all of them are checked. */
export async function downloadModels() {
  const repair = useCover.getState().damaged;
  useCover.setState({ failed: null, damaged: false });
  try {
    useCover.setState({ modelsJob: await api.startVisionModels(repair) });
  } catch (e) {
    useCover.setState({ failed: plainError(errorText(e)) });
  }
}

/** Why a cover job failed, in plain words; a model that does not load is damaged and is downloaded again. */
function failure(message: string | null): Partial<CoverState> {
  const text = message ?? "";
  if (/Loading \S+\.onnx/.test(text)) return { failed: "A cover model is damaged. Download the cover models again.", damaged: true };
  return { failed: plainError(text), damaged: false };
}

/** How the cover jobs end: Pick for me sets the best frame, a mask redraws the cover, a download checks the models again. */
export function onCoverJob(job: JobEvent) {
  const s = useCover.getState();
  if (job.status === "running") return;
  if (job.id === s.exportJob && !useEditor.getState().exportOpen) {
    const { toast } = useEditor.getState();
    if (job.status === "done") toast({ kind: "success", text: "Cover exported", action: job.output ? { label: "Show in folder", run: () => void revealItemInDir(job.output!) } : undefined });
    if (job.status === "failed") toast({ kind: "error", text: `Cover export failed: ${job.message}`, action: { label: "Details", run: () => useEditor.setState({ exportOpen: true }) } });
    if (job.status !== "failed") useCover.setState({ exportJob: null });
  }
  if (job.id === s.modelsJob || job.id === s.pickJob || job.id === s.maskJob) void loadModels();
  if (job.id === s.modelsJob) useCover.setState({ modelsJob: null, ...(job.status === "failed" ? failure(job.message) : { failed: null }) });
  if (job.id === s.maskJob) useCover.setState({ maskJob: null, masks: s.masks + 1, ...(job.status === "failed" ? failure(job.message) : {}) });
  if (job.id !== s.pickJob) return;
  useCover.setState({ pickJob: null, masks: s.masks + 1 });
  if (job.status === "failed") return useCover.setState(failure(job.message));
  if (job.status !== "done" || !job.output) return;
  const found = JSON.parse(job.output) as CoverPick;
  const epoch = currentEpoch();
  if (!epoch) return;
  useCover.setState({ picks: { ...useCover.getState().picks, [found.format]: { epoch, candidates: found.candidates } } });
  const best = found.candidates[0];
  const now = thumbnailOf(useEditor.getState().snap?.project, found.format)?.timeUs ?? null;
  const from = s.pickFrom?.format === found.format ? s.pickFrom.timeUs : now;
  if (!best) return;
  if (now !== from) return useCover.setState({ kept: found.format });
  void chooseFrame(found.format, best.timeUs);
  if (useCover.getState().open === found.format) useEditor.getState().seek(best.timeUs);
}

/** Candidates Pick for me found for `format` in the open project. */
export function useCandidates(format: ThumbnailFormat): CoverCandidate[] {
  const epoch = useEditor((s) => s.snap?.sessionEpoch);
  return useCover((s) => (s.picks[format]?.epoch === epoch ? s.picks[format]!.candidates : NONE));
}
const NONE: CoverCandidate[] = [];

const PART_WORDS: [string, string, number][] = [
  ["eyes_open", "Eyes open", 0.8],
  ["smile", "smiling", 0.5],
  ["facing", "facing you", 0.75],
  ["sharpness", "sharp", 0.6],
  ["exposure", "well lit", 0.7],
  ["mouth", "mouth at rest", 0.8],
];

/** The candidate's strongest parts in plain words, at most three: "Eyes open, sharp, smiling". */
export function candidateReason(c: CoverCandidate, format: ThumbnailFormat) {
  const framing = format === "cover_9x16" ? "framing_9x16" : "framing_16x9";
  const words = [...PART_WORDS, [framing, "well framed", 0.7] as [string, string, number]]
    .filter(([key, , min]) => (c.parts[key] ?? 0) >= min)
    .sort((a, b) => (c.parts[b[0]] ?? 0) - b[2] - ((c.parts[a[0]] ?? 0) - a[2]))
    .slice(0, 3)
    .map(([, word]) => word);
  if (c.face && (c.parts.eyes_open ?? 1) < 0.4) words.unshift("eyes closed");
  if (words.length === 0) return c.face ? "Face visible" : "No face, the clearest picture";
  const text = words.join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}
