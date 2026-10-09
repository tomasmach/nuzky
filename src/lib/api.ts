import { invoke } from "@tauri-apps/api/core";
import type { AgentConnection, AgentKind, Boot, Collection, DeletedCollection, SpeechModel, EditCmd, ExportRequest, Filmstrip, FontFamilies, LayerBounds, Library, ProjectSummary, Said, Snapshot, TextStyle, TranscriptCut, TranscriptView, WordsCorrected, ZoomSuggestions, ZoomsApplied } from "./types";

/**
 * The session a change was made for. Mutating commands carry it, so a change still in flight when
 * another project opens is refused instead of landing in the wrong project.
 */
type Epoch = string | undefined;

export const api = {
  setUiContext: (selection: string[], playheadUs: number) => invoke<void>("set_ui_context", { selection, playheadUs: Math.round(playheadUs) }),
  resolveRecovery: (action: "keep" | "restore", epoch: Epoch) => invoke<Snapshot>("resolve_recovery", { action, expectedEpoch: epoch }),
  /** Ends the agent's run, keeping its changes as one undo step; `discard` takes them back instead. */
  stopRun: (epoch: Epoch, discard = false) => invoke<Snapshot>("stop_run", { expectedEpoch: epoch, discard }),
  boot: () => invoke<Boot>("boot"),
  applyEdit: (cmd: EditCmd, coalesce: string | null, epoch: Epoch) => invoke<Snapshot>("apply_edit", { cmd, coalesce, expectedEpoch: epoch }),
  /** All or nothing, as one undo step. */
  applyEdits: (cmds: EditCmd[], coalesce: string | null, epoch: Epoch) => invoke<Snapshot>("apply_edits", { cmds, coalesce, expectedEpoch: epoch }),
  undo: (epoch: Epoch) => invoke<Snapshot>("undo", { expectedEpoch: epoch }),
  redo: (epoch: Epoch) => invoke<Snapshot>("redo", { expectedEpoch: epoch }),
  importMedia: (paths: string[], epoch: Epoch) =>
    invoke<{ snapshot: Snapshot; added: string[]; failed: { path: string; error: string }[] }>("import_media", { paths, expectedEpoch: epoch }),
  thumbnail: (assetId: string) => invoke<string | null>("thumbnail", { assetId }),
  waveform: (assetId: string) => invoke<number[] | null>("waveform", { assetId }),
  play: () => invoke<void>("play"),
  pause: () => invoke<void>("pause"),
  seek: (tUs: number) => invoke<void>("seek", { tUs: Math.round(tUs) }),
  setPreviewBox: (width: number, height: number) => invoke<void>("set_preview_box", { width, height }),
  listProjects: () => invoke<ProjectSummary[]>("list_projects"),
  agentConnections: () => invoke<AgentConnection[]>("agent_connections"),
  connectAgent: (agent: AgentKind) => invoke<AgentConnection>("connect_agent", { agent }),
  newProject: (width: number, height: number) => invoke<Snapshot>("new_project", { width, height }),
  openProject: (path: string) => invoke<Snapshot>("open_project", { path }),
  /** A new project with the media on its timeline, in the format of the first picture. */
  newProjectFromMedia: (paths: string[]) =>
    invoke<{ snapshot: Snapshot; added: string[]; failed: { path: string; error: string }[] }>("new_project_from_media", { paths }),
  library: () => invoke<Library>("library"),
  /** A PNG data URL of the project's picture, or null when it has none. */
  projectPoster: (path: string) => invoke<string | null>("project_poster", { path }),
  searchSaid: (query: string, collection: string | null = null) => invoke<Said>("search_said", { query, collection }),
  /** For a project that is not open; the open one is renamed with an edit. */
  renameProject: (path: string, name: string) => invoke<void>("rename_project", { path, name }),
  /** Returns the copy's path. */
  duplicateProject: (path: string) => invoke<string>("duplicate_project", { path }),
  trashProjects: (paths: string[]) => invoke<void>("trash_projects", { paths }),
  restoreProjects: (paths: string[]) => invoke<void>("restore_projects", { paths }),
  createCollection: (name: string) => invoke<Collection>("create_collection", { name }),
  renameCollection: (id: string, name: string) => invoke<void>("rename_collection", { id, name }),
  deleteCollection: (id: string) => invoke<DeletedCollection>("delete_collection", { id }),
  restoreCollection: (deleted: DeletedCollection) => invoke<void>("restore_collection", { deleted }),
  /** null takes the projects out of their collection. */
  setCollection: (paths: string[], collection: string | null) => invoke<void>("set_collection", { paths, collection }),
  /** Without `replaceExisting` an existing file is kept and the export fails with DESTINATION_EXISTS. */
  startExport: (path: string, options: ExportRequest, epoch: Epoch, replaceExisting: boolean) =>
    invoke<string>("start_export", { path, options, expectedEpoch: epoch, replaceExisting }),
  filmstrip: (assetId: string) => invoke<Filmstrip | null>("filmstrip", { assetId }),
  layerBounds: (tUs: number) => invoke<LayerBounds[]>("layer_bounds", { tUs: Math.round(tUs) }),
  cancelJob: (id: string) => invoke<void>("cancel_job", { id }),
  speechModels: () => invoke<SpeechModel[]>("speech_models"),
  /** `maxWords`/`maxChars` null group by phrase, capped at 12 words and 42 characters. */
  startCaptions: (model: string, language: string, style: TextStyle, maxWords: number | null, maxChars: number | null, epoch: Epoch) =>
    invoke<string>("start_captions", { request: { model, language, style, maxWords, maxChars }, expectedEpoch: epoch }),
  /** Recognises the heard media without a transcript, or all of it with `refresh`. */
  startTranscript: (model: string, language: string, refresh: boolean, epoch: Epoch) => invoke<string>("start_transcript", { model, language, refresh, expectedEpoch: epoch }),
  transcriptView: (pauseUs: number) => invoke<TranscriptView>("transcript_view", { pauseUs: Math.round(pauseUs) }),
  /** Inclusive word index ranges, as one undo step. */
  cutWords: (key: string, ranges: [number, number][], epoch: Epoch) => invoke<TranscriptCut>("cut_words", { key, delete: ranges, expectedEpoch: epoch }),
  /** The view's pauses at `pauseUs`, by index, or all of them. */
  removePauses: (key: string, pauseUs: number, only: number[] | null, epoch: Epoch) =>
    invoke<TranscriptCut>("remove_pauses", { key, pauseUs: Math.round(pauseUs), only, expectedEpoch: epoch }),
  /** Words by view index and the text they should read, with the captions that show them, as one undo step. */
  correctWords: (key: string, corrections: { i: number; text: string }[], epoch: Epoch) =>
    invoke<WordsCorrected>("correct_words", { key, corrections, expectedEpoch: epoch }),
  listFonts: () => invoke<FontFamilies>("list_fonts"),
  /** Sentences of the timeline said with emphasis, for a punch-in. */
  suggestZooms: () => invoke<ZoomSuggestions>("suggest_zooms"),
  /** Punch-ins on inclusive word ranges of the view with `key`, as one undo step. */
  applyZooms: (key: string, zooms: { from: number; to: number; scale: number }[], epoch: Epoch) =>
    invoke<ZoomsApplied>("apply_zooms", { key, zooms, expectedEpoch: epoch }),
};

/** The error as the backend sent it, "CODE: detail" included; code checks use this. */
export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** What the backend's error codes mean for someone editing, with what to do next. */
const PLAIN: Record<string, string | ((detail: string) => string)> = {
  READ_ONLY: "This project cannot be changed in this window. Open it again from Projects.",
  RECOVERY_PENDING: "Choose what happens to the unfinished AI edit first.",
  RUN_ACTIVE: "AI is editing. Stop it to edit yourself.",
  RUN_BUSY: "An AI edit is still open. Stop it first.",
  EPOCH_CHANGED: "Another project was opened meanwhile, so this did not happen.",
  EDIT_REJECTED: (detail) => {
    const what = detail.replace(/; project unchanged$/, "").replace(/\.$/, "");
    // A clip, track or media item that went away meanwhile, e.g. through Undo or an AI edit.
    const gone = /^unknown (clip|track|asset|media)\b/i.exec(what);
    if (gone) return `That ${gone[1] === "asset" ? "media item" : gone[1].toLowerCase()} was removed meanwhile, so nothing changed.`;
    return `${sentence(what)}. The project is unchanged.`;
  },
  INVALID_PROJECT: (detail) => `That change would break the project (${detail}), so it was not made.`,
  SPEECH_CHANGED: "The speech on the timeline changed meanwhile. Select the words again and retry.",
  TRANSCRIPT_MISSING: "Transcribe the timeline first, then try again.",
  AUDIO_NOT_READY: "The sound of the clips is still being prepared. Try again in a moment.",
  NO_WORDS: "Transcribe the timeline first, then try again.",
  OUTPUT_EXISTS: "A file with that name appeared while exporting. Export again to replace it or choose another name.",
  DESTINATION_EXISTS: "A file with that name already exists.",
  CANCELLED: "Cancelled.",
  AGENT_BUSY: (detail) => detail,
  NOT_INSTALLED: (detail) => sentence(detail.replace(/\.$/, "")) + ". Install it and sign in from a terminal.",
  APP_CLOSED: "CapOpen is closing, so this stopped.",
  JOB_FAILED: "The task stopped unexpectedly. Try again.",
  MODEL_INVALID: "The speech model did not download completely. Try again to download it anew.",
  STORE_UNREADABLE: "The saved transcripts could not be read. Transcribe the timeline again.",
};

/**
 * Text for people: a known "CODE: detail" becomes plain words with a next step, wherever it sits in
 * the message ("Export failed: CODE: …"); anything else is shown as it came.
 */
export function plainError(text: string): string {
  return text.replace(/\b([A-Z][A-Z_]{3,}): ?([\s\S]*)$/, (all, code: string, detail: string) => {
    const plain = PLAIN[code];
    return plain === undefined ? all : typeof plain === "string" ? plain : plain(detail);
  });
}
