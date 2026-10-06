// Mirrors of the Rust engine types (serde camelCase).

export type AssetKind = "video" | "audio" | "image";

export interface Asset {
  id: string;
  name: string;
  path: string;
  kind: AssetKind;
  durationUs: number;
  width: number;
  height: number;
  fps: number;
  hasAudio: boolean;
  rotation: number;
}

export interface Transform {
  x: number;
  y: number;
  scale: number;
  rotation: number;
  opacity: number;
}

export interface TextStyle {
  fontSize: number;
  color: string;
  bold: boolean;
  strokeWidth: number;
  strokeColor: string;
  background: string | null;
}

export type ClipContent =
  | { type: "media"; assetId: string; sourceInUs: number; volume: number; transform: Transform }
  | { type: "text"; text: string; style: TextStyle; transform: Transform };

export interface Clip {
  id: string;
  startUs: number;
  durationUs: number;
  content: ClipContent;
}

export type TrackKind = "video" | "audio" | "text";

export interface Track {
  id: string;
  kind: TrackKind;
  name: string;
  muted: boolean;
  hidden: boolean;
  clips: Clip[];
}

export interface Canvas {
  width: number;
  height: number;
  fps: number;
  background: string;
}

export interface Project {
  version: number;
  name: string;
  canvas: Canvas;
  assets: Asset[];
  tracks: Track[];
}

export interface Snapshot {
  project: Project;
  revision: number;
  canUndo: boolean;
  canRedo: boolean;
  path: string;
  select: string[];
}

export interface Transport {
  playing: boolean;
  tUs: number;
}

export interface Boot {
  snapshot: Snapshot;
  previewUrl: string;
  transport: Transport;
}

export interface JobEvent {
  id: string;
  kind: "audio" | "export" | "captions";
  label: string;
  status: "running" | "done" | "failed" | "cancelled";
  progress: number;
  phase: string | null;
  message: string | null;
  output: string | null;
}

export interface ProjectSummary {
  path: string;
  name: string;
  modifiedMs: number;
  durationUs: number;
}

export interface CaptionModel {
  id: string;
  label: string;
  sizeMb: number;
  downloaded: boolean;
}

export interface CaptionSegment {
  startUs: number;
  endUs: number;
  text: string;
}

export type EditCmd =
  | { type: "removeAsset"; assetId: string }
  | { type: "addClip"; assetId: string; startUs: number | null; trackId: string | null }
  | { type: "addText"; startUs: number; text: string; style: TextStyle }
  | { type: "moveClip"; clipId: string; trackId: string | null; startUs: number }
  | { type: "trimClip"; clipId: string; startUs: number; durationUs: number; sourceInUs: number | null }
  | { type: "splitClip"; clipId: string; atUs: number }
  | { type: "deleteClips"; clipIds: string[] }
  | {
      type: "updateClip";
      clipId: string;
      transform?: Transform | null;
      volume?: number | null;
      text?: string | null;
      style?: TextStyle | null;
    }
  | { type: "updateTrack"; trackId: string; muted?: boolean | null; hidden?: boolean | null }
  | { type: "setCanvas"; width: number; height: number; background?: string | null }
  | { type: "renameProject"; name: string };
