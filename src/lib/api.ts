import { invoke } from "@tauri-apps/api/core";
import type { Boot, CaptionModel, EditCmd, ExportRequest, Filmstrip, ProjectSummary, Snapshot, TextStyle } from "./types";

export const api = {
  boot: () => invoke<Boot>("boot"),
  applyEdit: (cmd: EditCmd, coalesce?: string) => invoke<Snapshot>("apply_edit", { cmd, coalesce: coalesce ?? null }),
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
  cancelJob: (id: string) => invoke<void>("cancel_job", { id }),
  captionModels: () => invoke<CaptionModel[]>("caption_models"),
  startCaptions: (model: string, language: string, style: TextStyle) =>
    invoke<string>("start_captions", { request: { model, language, style } }),
};

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
