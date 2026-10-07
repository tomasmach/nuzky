import { useEffect, useState } from "react";
import { create } from "zustand";
import { api, errorText, plainError } from "./api";
import { aiLocked, currentEpoch, enqueue, undoAction, useEditor, whenIdle } from "./store";
import type { Project, TextStyle, TranscriptCut, TranscriptView, WordsCorrected } from "./types";

/** Characters per caption when captions show 1–3 words, like reels. */
export const SHORT_CAPTION_CHARS = 15;
/** Longest corrected word, as the engine's MAX_CORRECTION_CHARS. */
export const MAX_WORD_CHARS = 100;
/** A gap this long between two words starts a new paragraph in the transcript. */
const PARAGRAPH_GAP_US = 1_000_000;

/** Languages offered for recognition, as Whisper codes. */
export const SPEECH_LANGUAGES: [string, string][] = [
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

const LANGUAGE_KEY = "capopen.speechLanguage";

/** The language picked last, else the computer's language when it is offered, else detection. */
function initialLanguage(): string {
  const offered = (id: string | null | undefined) => !!id && SPEECH_LANGUAGES.some(([code]) => code === id);
  const saved = localStorage.getItem(LANGUAGE_KEY);
  if (offered(saved)) return saved!;
  const system = (navigator.languages?.[0] ?? navigator.language ?? "").slice(0, 2).toLowerCase();
  return offered(system) ? system : "auto";
}

/** Picks the spoken language for Captions and Transcript and remembers it for next time. */
export function setSpeechLanguage(language: string) {
  localStorage.setItem(LANGUAGE_KEY, language);
  useSpeech.setState({ language });
}

interface SpeechState {
  /** Shared by Captions and Transcript, so both use the same recognition. */
  model: string;
  language: string;
  /** 1–3 words per caption, or 0 for whole phrases. */
  captionWords: number;
  /** Appearance of new captions while none are on the timeline: preset index and font. */
  captionStyle: number;
  captionFont: string | null;
  /** Pauses longer than this are shown in the transcript and can be removed. */
  pauseUs: number;
  /** Counts changes to the stored transcripts, so the transcript is fetched again. */
  stored: number;
}

export const useSpeech = create<SpeechState>(() => ({
  model: "large-v3-turbo-q5_0",
  language: initialLanguage(),
  captionWords: 2,
  captionStyle: 0,
  captionFont: null,
  pauseUs: 500_000,
  stored: 0,
}));

/** Why the timeline cannot be transcribed, or null. Same rule as the engine's `is_heard`. */
export function speechBlocker(project: Project): string | null {
  const heard = project.tracks.some(
    (t) =>
      !t.muted &&
      t.clips.some((c) => {
        const m = c.content;
        if (m.type !== "media" || m.volume <= 0) return false;
        const asset = project.assets.find((a) => a.id === m.assetId);
        return asset?.kind === "video" && asset.hasAudio;
      }),
  );
  return heard ? null : "Add a video with sound to the timeline first";
}

const sameView = (a: TranscriptView, b: TranscriptView) => a.key === b.key && JSON.stringify([a.pauses, a.untranscribed]) === JSON.stringify([b.pauses, b.untranscribed]);

/**
 * The timeline's speech as it is now, fetched again after every edit, recognition or change of the
 * pause length. An edit that leaves speech alone keeps the same view, so the selection survives it.
 */
export function useTranscriptView(): { view: TranscriptView | null; error: string | null } {
  const revision = useEditor((s) => s.snap?.revision);
  const epoch = useEditor((s) => s.snap?.sessionEpoch);
  const pauseUs = useSpeech((s) => s.pauseUs);
  const stored = useSpeech((s) => s.stored);
  const [state, setState] = useState<{ epoch?: string; view: TranscriptView | null; error: string | null }>({ view: null, error: null });
  useEffect(() => {
    let live = true;
    api.transcriptView(pauseUs).then(
      (view) => live && setState((s) => ({ epoch, view: s.epoch === epoch && s.view && sameView(s.view, view) ? s.view : view, error: null })),
      (e) => live && setState({ epoch, view: null, error: plainError(errorText(e)) }),
    );
    return () => {
      live = false;
    };
  }, [revision, epoch, pauseUs, stored]);
  // Another project's transcript never shows, not even until the new one arrives.
  return state.epoch === epoch ? state : { view: null, error: null };
}

/** Starts recognition for the transcript, of every heard file with `refresh`, or for captions with `style`. */
export async function startSpeech(captions: { style: TextStyle } | null, refresh = false) {
  // Captions change the project; a transcript only reads it.
  if (captions && aiLocked()) return;
  const { model, language, captionWords } = useSpeech.getState();
  // Taken before waiting, so a project opened meanwhile refuses the request instead of running it.
  const epoch = currentEpoch();
  // The backend transcribes the project as it is when the job starts; wait for queued edits.
  await whenIdle();
  try {
    if (captions) {
      const words = captionWords || null;
      await api.startCaptions(model, language, captions.style, words, words ? SHORT_CAPTION_CHARS : null, epoch);
    } else await api.startTranscript(model, language, refresh, epoch);
  } catch (e) {
    useEditor.getState().toast({ kind: "error", text: errorText(e) });
  }
}

/**
 * Runs a transcript cut after the edits queued before it, as one undo step, and offers Undo.
 * `what` describes it from the time removed. Resolves to null when nothing was cut.
 */
async function cut(run: (epoch: string | undefined) => Promise<TranscriptCut>, what: (removedUs: number) => string): Promise<TranscriptCut | null> {
  if (aiLocked()) return null;
  const out: { done?: TranscriptCut } = {};
  await enqueue(async (epoch) => (out.done = await run(epoch)).snapshot);
  if (!out.done) return null;
  useEditor.getState().toast({ kind: "info", text: `Removed ${what(out.done.removedUs)}`, action: undoAction(out.done.snapshot) });
  return out.done;
}

export const cutWords = (key: string, ranges: [number, number][], what: (removedUs: number) => string) => cut((epoch) => api.cutWords(key, ranges, epoch), what);

export const removePauses = (key: string, pauseUs: number, only: number[] | null, what: (removedUs: number) => string) =>
  cut((epoch) => api.removePauses(key, pauseUs, only, epoch), what);

/**
 * Corrects how a word reads, in the transcript and in the captions that show it, after the edits
 * queued before it, as one undo step, and offers Undo. Resolves to whether it was corrected.
 */
export async function correctWord(key: string, i: number, text: string, shown: string): Promise<boolean> {
  if (aiLocked()) return false;
  const out: { done?: WordsCorrected } = {};
  await enqueue(async (epoch) => (out.done = await api.correctWords(key, [{ i, text }], epoch)).snapshot);
  if (!out.done) return false;
  const n = out.done.captions;
  const captions = n > 0 ? `, also in ${n === 1 ? "its caption" : `${n} captions`}` : "";
  useEditor.getState().toast({ kind: "info", text: `Corrected “${shown}” to “${text}”${captions}`, action: undoAction(out.done.snapshot) });
  return true;
}

export type Token =
  /** `original` is what recognition wrote, when the word was corrected. */
  | { kind: "word"; i: number; startUs: number; endUs: number; text: string; original?: string }
  /** The part of a pause that removing it cuts; `i` is its place in the view's pauses. */
  | { kind: "pause"; i: number; startUs: number; endUs: number; gapUs: number };

/** Words and pauses in timeline order. */
export function tokenize(view: TranscriptView): Token[] {
  const words = view.words.map((w): Token => ({ kind: "word", i: w.i, startUs: w.startUs, endUs: w.endUs, text: w.text, original: w.original }));
  const pauses = view.pauses.map((p, i): Token => ({ kind: "pause", i, ...p }));
  return [...words, ...pauses].sort((a, b) => a.startUs - b.startUs);
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
