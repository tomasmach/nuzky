import { useMemo } from "react";
import { create } from "zustand";
import { api, errorText } from "./api";
import { US } from "./time";
import { MAIN_TRACK, rippleDelete, useEditor, whenIdle } from "./store";
import type { Project, TextStyle, TimeRange, TimelineTranscript, TranscriptWord } from "./types";

/** Characters per caption when captions show 1–3 words, like reels. */
export const SHORT_CAPTION_CHARS = 15;
/** A gap this long between two words starts a new paragraph in the transcript. */
const PARAGRAPH_GAP_US = 1_000_000;
const MAX_VERSIONS = 30;

/** Words timed against one state of the speech on the timeline. */
interface TranscriptVersion {
  sig: string;
  language: string;
  words: TranscriptWord[];
}

interface SpeechState {
  /** Shared by Captions and Transcript, so both use the same recognition. */
  model: string;
  language: string;
  /** 1–3 words per caption, or 0 for whole phrases. */
  captionWords: number;
  /** Look for new captions while none are on the timeline: preset index and font. */
  captionStyle: number;
  captionFont: string | null;
  /**
   * Transcripts by speech timing. A cut made from the transcript adds the remapped words, so
   * undo and redo land on a version that is still valid instead of needing a new transcript.
   */
  versions: TranscriptVersion[];
  /** Speech timing when the running recognition job started; its words are timed against it. */
  pendingSig: string | null;
  /** Pauses longer than this are shown in the transcript and can be removed. */
  pauseUs: number;
}

export const useSpeech = create<SpeechState>(() => ({
  model: "large-v3-turbo-q5_0",
  language: "cs",
  captionWords: 2,
  captionStyle: 0,
  captionFont: null,
  versions: [],
  pendingSig: null,
  pauseUs: 500_000,
}));

/** Why the timeline cannot be transcribed, or null; the same check as the backend. */
export function speechBlocker(project: Project): string | null {
  const heard = project.tracks.some(
    (t) =>
      t.kind === "video" &&
      !t.muted &&
      t.clips.some((c) => {
        const m = c.content;
        return m.type === "media" && !!project.assets.find((a) => a.id === m.assetId)?.hasAudio;
      }),
  );
  return heard ? null : "Add a video with sound to the timeline first";
}

/**
 * Everything that decides when a word is heard on the timeline: the sound of video tracks, the
 * same mix recognition listens to. Captions, text, transforms, grading or music leave it alone,
 * so they never make a transcript stale.
 */
export function speechSig(project: Project): string {
  return JSON.stringify(
    project.tracks
      .filter((t) => t.kind === "video")
      .map((t) => [
        t.id,
        t.muted,
        t.clips.map((c) => (c.content.type === "media" ? [c.startUs, c.durationUs, c.content.assetId, c.content.sourceInUs, c.content.speed, c.content.volume > 0] : [])),
      ]),
  );
}

/**
 * The transcript that matches the project as it is now. When none does but an older one exists,
 * `stale` is set and `words` holds the newest one, for display only.
 */
export function useTranscript(): { words: TranscriptWord[] | null; language: string | null; stale: boolean } {
  const project = useEditor((s) => s.snap?.project);
  const versions = useSpeech((s) => s.versions);
  const sig = useMemo(() => (project ? speechSig(project) : ""), [project]);
  const v = versions.find((x) => x.sig === sig) ?? versions[versions.length - 1];
  return { words: v?.words ?? null, language: v?.language ?? null, stale: !!v && v.sig !== sig };
}

function addVersion(v: TranscriptVersion) {
  const rest = useSpeech.getState().versions.filter((x) => x.sig !== v.sig);
  useSpeech.setState({ versions: [...rest, v].slice(-MAX_VERSIONS) });
}

/** Starts recognition for a transcript, or for captions with `style`. */
export async function startSpeech(captions: { style: TextStyle } | null) {
  const { model, language, captionWords, pendingSig } = useSpeech.getState();
  // The backend transcribes the project as it is when the job starts; wait for queued edits.
  await whenIdle();
  const snap = useEditor.getState().snap;
  if (!snap) return;
  useSpeech.setState({ pendingSig: speechSig(snap.project) });
  try {
    if (captions) {
      const words = captionWords || null;
      await api.startCaptions(model, language, captions.style, words, words ? SHORT_CAPTION_CHARS : null);
    } else await api.startTranscript(model, language);
  } catch (e) {
    useSpeech.setState({ pendingSig });
    useEditor.getState().toast({ kind: "error", text: errorText(e) });
  }
}

/** `transcript-ready`: words of the job that just finished (captions jobs send them too). */
export function receiveTranscript(t: TimelineTranscript) {
  const { pendingSig } = useSpeech.getState();
  const snap = useEditor.getState().snap;
  const sig = pendingSig ?? (snap && snap.revision === t.revision ? speechSig(snap.project) : null);
  useSpeech.setState({ pendingSig: null });
  if (sig && t.words.length > 0) addVersion({ sig, language: t.language, words: t.words });
}

export function speechJobEnded() {
  useSpeech.setState({ pendingSig: null });
}

/** Picks up the transcript the backend still holds for this exact project revision, e.g. after a reload. */
export async function loadStoredTranscript() {
  await whenIdle();
  const t = await api.getTranscript().catch(() => null);
  const snap = useEditor.getState().snap;
  if (t && snap && t.revision === snap.revision && t.words.length > 0) addVersion({ sig: speechSig(snap.project), language: t.language, words: t.words });
}

/** Sorted, non-overlapping, non-empty; the same normalisation as the engine. */
function mergeRanges(ranges: TimeRange[]): TimeRange[] {
  const sorted = ranges
    .filter((r) => r.endUs > Math.max(0, r.startUs))
    .map((r) => ({ startUs: Math.max(0, Math.round(r.startUs)), endUs: Math.round(r.endUs) }))
    .sort((a, b) => a.startUs - b.startUs);
  const out: TimeRange[] = [];
  for (const r of sorted) {
    const last = out[out.length - 1];
    if (last && r.startUs <= last.endUs) last.endUs = Math.max(last.endUs, r.endUs);
    else out.push({ ...r });
  }
  return out;
}

/** Where a time lands once `ranges` are cut out and the gaps closed. */
function mapTime(t: number, ranges: TimeRange[]): number {
  let shift = 0;
  for (const r of ranges) {
    if (t >= r.endUs) shift += r.endUs - r.startUs;
    else return (t > r.startUs ? r.startUs : t) - shift;
  }
  return t - shift;
}

/** Words after a ripple delete: words mostly inside a range go, later words move left. */
export function remapWords(words: TranscriptWord[], ranges: TimeRange[]): TranscriptWord[] {
  const rs = mergeRanges(ranges);
  return words
    .filter((w) => {
      const mid = (w.startUs + w.endUs) / 2;
      return !rs.some((r) => mid >= r.startUs && mid < r.endUs);
    })
    .map((w) => ({ ...w, startUs: mapTime(w.startUs, rs), endUs: mapTime(w.endUs, rs) }));
}

/**
 * Cuts `ranges` out of the timeline, but only while the transcript they came from still matches
 * the project when the edit is sent. Resolves to false when nothing was cut.
 */
export async function cutFromTranscript(ranges: TimeRange[], what: string): Promise<boolean> {
  const { edit, toast, undo } = useEditor.getState();
  // Checked inside the builder, against the project the edit is applied to.
  const used: { source?: TranscriptVersion } = {};
  const snap = await edit((project) => {
    const sig = speechSig(project);
    used.source = useSpeech.getState().versions.find((v) => v.sig === sig);
    return used.source ? rippleDelete(project, ranges) : null;
  });
  const source = used.source;
  if (!source) {
    toast({ kind: "error", text: "The video changed after the transcript was made. Refresh the transcript first." });
    return false;
  }
  if (!snap) return false;
  const sig = speechSig(snap.project);
  if (sig !== source.sig) addVersion({ sig, language: source.language, words: remapWords(source.words, ranges) });
  toast({ kind: "info", text: `Removed ${what}`, action: { label: "Undo", run: undo } });
  return true;
}

export type Token =
  | { kind: "word"; startUs: number; endUs: number; text: string; probability: number }
  /** The part of a pause longer than the threshold; half the threshold stays next to each word. */
  | { kind: "pause"; startUs: number; endUs: number; gapUs: number };

/**
 * Where pauses may be cut: main-track clips recognition hears (the rule of `speechSig`) that hold at least one
 * word. B-roll without words, even with ambient sound, is never part of a pause.
 */
export function speechRanges(project: Project, words: TranscriptWord[]): TimeRange[] {
  // Main track only: a word's time alone cannot tell whether an overlapping overlay, such as B-roll
  // with ambient sound, is where it was said.
  const heard = project.tracks
    .filter((t) => t.id === MAIN_TRACK && !t.muted)
    .flatMap((t) => t.clips)
    .filter((c) => {
      const m = c.content;
      if (m.type !== "media" || m.volume <= 0) return false;
      const asset = project.assets.find((a) => a.id === m.assetId);
      return asset?.kind === "video" && asset.hasAudio;
    })
    .map((c) => ({ startUs: c.startUs, endUs: c.startUs + c.durationUs }));
  const mids = words.map((w) => (w.startUs + w.endUs) / 2);
  return mergeRanges(heard.filter((r) => mids.some((t) => t >= r.startUs && t < r.endUs)));
}

/** Words plus a pause token wherever the silence inside `speech` is longer than `pauseUs`. */
export function tokenize(words: TranscriptWord[], pauseUs: number, speech: TimeRange[]): Token[] {
  const pad = pauseUs / 2;
  const out: Token[] = [];
  // The silence between `from` and `to` (null: the timeline's edge), split where it leaves speech.
  // Only the sides next to a word keep half the threshold.
  const pauses = (from: number | null, to: number | null) => {
    for (const r of speech) {
      const start = Math.max(r.startUs, from ?? -Infinity);
      const end = Math.min(r.endUs, to ?? Infinity);
      const startUs = start === from ? start + pad : start;
      const endUs = end === to ? end - pad : end;
      if (end - start > pauseUs && endUs > startUs) out.push({ kind: "pause", startUs, endUs, gapUs: end - start });
    }
  };
  words.forEach((w, i) => {
    pauses(i === 0 ? null : words[i - 1].endUs, w.startUs);
    out.push({ kind: "word", startUs: w.startUs, endUs: w.endUs, text: w.text, probability: w.probability });
  });
  const last = words[words.length - 1];
  if (last) pauses(last.endUs, null);
  return out;
}

/** Token index ranges [from, to) of paragraphs, split where speech pauses for a second or more. */
export function paragraphs(tokens: Token[]): [number, number][] {
  const out: [number, number][] = [];
  let start = 0;
  let lastWordEnd: number | null = null;
  tokens.forEach((t, i) => {
    if (t.kind !== "word") return;
    if (lastWordEnd !== null && t.startUs - lastWordEnd >= PARAGRAPH_GAP_US && i > start) {
      out.push([start, i]);
      start = i;
    }
    lastWordEnd = t.endUs;
  });
  if (start < tokens.length) out.push([start, tokens.length]);
  return out;
}

/** Index of the token playing at `t`, or -1. A word stays current through a short gap after it. */
export function tokenAt(tokens: Token[], t: number): number {
  let lo = 0;
  let hi = tokens.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (tokens[mid].startUs <= t) {
      found = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  if (found < 0) return -1;
  const next = tokens[found + 1];
  return t < tokens[found].endUs || (tokens[found].kind === "word" && next?.kind === "word") ? found : -1;
}

export function formatSeconds(us: number) {
  return `${(us / US).toFixed(1)} s`;
}
