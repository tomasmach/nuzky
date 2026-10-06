import { invoke } from "@tauri-apps/api/core";
import type { Boot, CaptionModel, EditCmd, ExportRequest, Filmstrip, FontFamilies, LayerBounds, ProjectSummary, Snapshot, TextStyle, TranscriptCut, TranscriptView } from "./types";

export const api = {
  setUiContext: (selection: string[], playheadUs: number) => invoke<void>("set_ui_context", { selection, playheadUs: Math.round(playheadUs) }),
  resolveRecovery: (action: "keep" | "restore") => invoke<Snapshot>("resolve_recovery", { action }),
  stopRun: () => invoke<Snapshot>("stop_run"),
  boot: () => invoke<Boot>("boot"),
  applyEdit: (cmd: EditCmd, coalesce?: string, expectRevision?: number, expectSpeechKey?: string) => invoke<Snapshot>("apply_edit", { cmd, coalesce: coalesce ?? null, expectRevision, expectSpeechKey }),
  /** All or nothing, as one undo step. */
  applyEdits: (cmds: EditCmd[], coalesce?: string, expectRevision?: number, expectSpeechKey?: string) => invoke<Snapshot>("apply_edits", { cmds, coalesce: coalesce ?? null, expectRevision, expectSpeechKey }),
  undo: () => invoke<Snapshot>("undo"),
  redo: () => invoke<Snapshot>("redo"),
  importMedia: (paths: string[]) =>
    invoke<{ snapshot: Snapshot; added: string[]; failed: { path: string; error: string }[] }>("import_media", { paths }),
  thumbnail: (assetId: string) => invoke<string | null>("thumbnail", { assetId }),
  waveform: (assetId: string) => invoke<number[] | null>("waveform", { assetId }),
  play: () => invoke<void>("play"),
  pause: () => invoke<void>("pause"),
  seek: (tUs: number) => invoke<void>("seek", { tUs: Math.round(tUs) }),
  setPreviewBox: (width: number, height: number) => invoke<void>("set_preview_box", { width, height }),
  listProjects: () => invoke<ProjectSummary[]>("list_projects"),
  newProject: (width: number, height: number) => invoke<Snapshot>("new_project", { width, height }),
  openProject: (path: string) => invoke<Snapshot>("open_project", { path }),
  startExport: (path: string, options: ExportRequest) => invoke<string>("start_export", { path, options }),
  filmstrip: (assetId: string) => invoke<Filmstrip | null>("filmstrip", { assetId }),
  layerBounds: (tUs: number) => invoke<LayerBounds[]>("layer_bounds", { tUs: Math.round(tUs) }),
  cancelJob: (id: string) => invoke<void>("cancel_job", { id }),
  captionModels: () => invoke<CaptionModel[]>("caption_models"),
  /** `maxWords`/`maxChars` null keep whole phrases. */
  startCaptions: (model: string, language: string, style: TextStyle, maxWords: number | null, maxChars: number | null) =>
    invoke<string>("start_captions", { request: { model, language, style, maxWords, maxChars } }),
  /** Recognises the heard media without a transcript, or all of it with `refresh`. */
  startTranscript: (model: string, language: string, refresh: boolean) => invoke<string>("start_transcript", { model, language, refresh }),
  transcriptView: (pauseUs: number) => invoke<TranscriptView>("transcript_view", { pauseUs: Math.round(pauseUs) }),
  /** Inclusive word index ranges, as one undo step. */
  cutWords: (key: string, ranges: [number, number][]) => invoke<TranscriptCut>("cut_words", { key, delete: ranges }),
  /** The view's pauses at `pauseUs`, by index, or all of them. */
  removePauses: (key: string, pauseUs: number, only: number[] | null) => invoke<TranscriptCut>("remove_pauses", { key, pauseUs: Math.round(pauseUs), only }),
  listFonts: () => invoke<FontFamilies>("list_fonts"),
};

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
