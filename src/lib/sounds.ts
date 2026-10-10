import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { api, errorText } from "./api";
import { aiLocked, currentEpoch, useEditor } from "./store";
import type { Sound, SoundKind } from "./types";

export type AudioTab = "project" | "music" | "effects";

export type Search = {
  query: string;
  sounds: Sound[];
  page: number;
  more: boolean;
  /** "Openverse" or "Freesound", once results came. */
  service: string | null;
  loading: boolean;
  error: string | null;
};

const EMPTY: Search = { query: "", sounds: [], page: 0, more: false, service: null, loading: false, error: null };

type State = {
  tab: AudioTab;
  builtIn: Sound[];
  search: Record<SoundKind, Search>;
  /** The sound playing, or still loading to play. */
  preview: { id: string; loading: boolean } | null;
  /** Download progress of a sound being fetched, 0 to 1. */
  progress: Record<string, number>;
  /** Sounds being added to the timeline. */
  adding: string[];
};

export const useSounds = create<State>(() => ({
  tab: "project",
  builtIn: [],
  search: { music: EMPTY, effect: EMPTY },
  preview: null,
  progress: {},
  adding: [],
}));

let listening = false;
/** Download progress and finished previews come as events; the timeline starting to play stops a preview. */
function listenOnce() {
  if (listening) return;
  listening = true;
  listen<{ id: string; progress: number }>("sound-progress", (e) =>
    useSounds.setState((s) => ({ progress: { ...s.progress, [e.payload.id]: e.payload.progress } })),
  );
  listen<string>("sound-preview-ended", (e) => {
    if (useSounds.getState().preview?.id === e.payload) useSounds.setState({ preview: null });
  });
  useEditor.subscribe((s, prev) => {
    if (s.playing && !prev.playing && useSounds.getState().preview) stopPreview();
  });
}

export async function loadBuiltIn() {
  listenOnce();
  if (useSounds.getState().builtIn.length === 0) useSounds.setState({ builtIn: await api.soundLibrary() });
}

const searches: Record<SoundKind, number> = { music: 0, effect: 0 };
const setSearch = (kind: SoundKind, next: Partial<Search>) => useSounds.setState((s) => ({ search: { ...s.search, [kind]: { ...s.search[kind], ...next } } }));

/** Page 1 replaces the results, a later page adds to them. An answer to an older search is dropped. */
export async function searchSounds(kind: SoundKind, query: string, page = 1) {
  const seq = ++searches[kind];
  const text = query.trim();
  if (!text) {
    setSearch(kind, { ...EMPTY, query });
    return;
  }
  setSearch(kind, { query, loading: true, error: null, ...(page === 1 ? { sounds: [], page: 0, more: false } : {}) });
  try {
    const found = await api.soundSearch(text, kind, page);
    if (seq !== searches[kind]) return;
    const before = page === 1 ? [] : useSounds.getState().search[kind].sounds;
    setSearch(kind, { sounds: [...before, ...found.sounds], page, more: found.more, service: found.service, loading: false });
  } catch (e) {
    if (seq === searches[kind]) setSearch(kind, { loading: false, error: errorText(e) });
  }
}

export function stopPreview() {
  useSounds.setState({ preview: null });
  api.soundStop();
}

/** Plays the sound, or stops it when it is the one playing. The timeline pauses meanwhile. */
export async function togglePreview(sound: Sound) {
  if (useSounds.getState().preview?.id === sound.id) return stopPreview();
  const { playing, toast } = useEditor.getState();
  if (playing) await api.pause();
  useSounds.setState({ preview: { id: sound.id, loading: true } });
  try {
    await api.soundPreview(sound.id);
    if (useSounds.getState().preview?.id === sound.id) useSounds.setState({ preview: { id: sound.id, loading: false } });
  } catch (e) {
    const text = errorText(e);
    if (useSounds.getState().preview?.id !== sound.id || text.startsWith("CANCELLED")) return;
    useSounds.setState({ preview: null });
    toast({ kind: "error", text });
  }
}

/** Adds the sound at the playhead, downloading it first when needed. */
export async function addSound(sound: Sound) {
  if (aiLocked() || useSounds.getState().adding.includes(sound.id)) return;
  const { setSnap, toast, timeUs } = useEditor.getState();
  useSounds.setState((s) => ({ adding: [...s.adding, sound.id] }));
  try {
    setSnap(await api.soundAdd(sound.id, timeUs, currentEpoch()));
  } catch (e) {
    const text = errorText(e);
    if (!text.includes("CANCELLED")) toast({ kind: "error", text: `Could not add ${sound.title}: ${text}` });
  } finally {
    useSounds.setState((s) => ({ adding: s.adding.filter((id) => id !== sound.id), progress: { ...s.progress, [sound.id]: 0 } }));
  }
}

/** WebKitGTK may refuse the asynchronous clipboard; a selected text area always copies. */
async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.cssText = "position:fixed;opacity:0";
    document.body.append(area);
    area.select();
    const copied = document.execCommand("copy");
    area.remove();
    if (!copied) throw new Error("The clipboard is not available. Export the video: its credits are saved beside it.");
  }
}

export async function copyCredits() {
  const { toast } = useEditor.getState();
  try {
    const text = await api.soundCredits(currentEpoch());
    if (!text) return toast({ kind: "info", text: "No sound in the video needs credit." });
    await copyText(text);
    toast({ kind: "success", text: "Credits copied. Paste them into the video's description." });
  } catch (e) {
    toast({ kind: "error", text: errorText(e) });
  }
}
