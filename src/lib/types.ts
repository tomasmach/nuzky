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
  /** Font family; null or missing uses the bundled default, Inter. */
  fontFamily?: string | null;
  fontSize: number;
  color: string;
  bold: boolean;
  strokeWidth: number;
  strokeColor: string;
  background: string | null;
}

/** All 0 = unchanged. -1..1, vignette 0..1. */
export interface Adjust {
  brightness: number;
  contrast: number;
  saturation: number;
  temperature: number;
  vignette: number;
}

export type ClipContent =
  | {
      type: "media";
      assetId: string;
      sourceInUs: number;
      volume: number;
      transform: Transform;
      /** Clip covers durationUs * speed of source; 0.1..10 */
      speed: number;
      adjust: Adjust;
      fadeInUs: number;
      fadeOutUs: number;
    }
  | { type: "text"; text: string; style: TextStyle; transform: Transform };

export type AnimationKind = "fade" | "zoomIn" | "zoomOut" | "slideUp" | "slideDown" | "slideLeft" | "slideRight" | "pop" | "typewriter";
export interface Animation {
  kind: AnimationKind;
  durationUs: number;
}

/** tUs is relative to the clip start. When keyframes exist they replace content.transform. */
export interface Keyframe {
  tUs: number;
  transform: Transform;
}

export type TransitionKind = "dissolve" | "fadeBlack" | "fadeWhite" | "slideLeft" | "slideUp" | "zoomIn" | "wipeLeft" | "blur";
/** Centred on the cut before the clip that owns it; main track only, never on the first clip. */
export interface Transition {
  kind: TransitionKind;
  durationUs: number;
}

export interface Clip {
  id: string;
  startUs: number;
  durationUs: number;
  content: ClipContent;
  animIn: Animation | null;
  animOut: Animation | null;
  keyframes: Keyframe[];
  transitionIn: Transition | null;
}

export interface Filmstrip {
  url: string;
  frameWidth: number;
  frameHeight: number;
  intervalUs: number;
  count: number;
}

/** Corners tl, tr, br, bl in canvas pixels; list is bottom to top. */
export interface LayerBounds {
  clipId: string;
  corners: [number, number][];
}

export interface ExportRequest {
  /** Short side in px: 720, 1080, 1440, 2160 */
  resolution: number;
  fps: number;
  quality: "high" | "recommended" | "small";
}

export type TrackKind = "video" | "audio" | "text";

export interface Track {
  id: string;
  kind: TrackKind;
  name: string;
  muted: boolean;
  hidden: boolean;
  /** Ripple cuts leave the track alone; on for music, off for sound detached from a video. */
  keepInPlace: boolean;
  clips: Clip[];
}

export interface Canvas {
  width: number;
  height: number;
  fps: number;
  background: string;
  /** 0 = solid colour; above 0 a blurred copy of the main-track frame fills the background. 0..1 */
  backgroundBlur: number;
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
  kind: "audio" | "export" | "captions" | "transcript";
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

/** Families the engine can draw: bundled ones ship with CapOpen, system ones are installed here. */
export interface FontFamilies {
  bundled: string[];
  system: string[];
}

/** A recognised word in timeline time. */
export interface TranscriptWord {
  startUs: number;
  endUs: number;
  text: string;
  probability: number;
}

/** Words of the whole timeline, valid for the project `revision` it was made from. */
export interface TimelineTranscript {
  revision: number;
  language: string;
  words: TranscriptWord[];
}

/** Timeline range [startUs, endUs). */
export interface TimeRange {
  startUs: number;
  endUs: number;
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
      speed?: number | null;
      adjust?: Adjust | null;
      fadeInUs?: number | null;
      fadeOutUs?: number | null;
    }
  | { type: "setAnimation"; clipId: string; slot: "in" | "out"; animation: Animation | null }
  | { type: "setTransition"; clipId: string; transition: Transition | null }
  | { type: "setKeyframes"; clipId: string; keyframes: Keyframe[] }
  | { type: "duplicateClip"; clipId: string }
  | { type: "detachAudio"; clipId: string }
  | { type: "updateTrack"; trackId: string; muted?: boolean | null; hidden?: boolean | null; keepInPlace?: boolean | null }
  | { type: "addCaptions"; segments: CaptionSegment[]; style: TextStyle }
  | { type: "replaceCaptions"; trackId: string; segments: CaptionSegment[]; style: TextStyle }
  /** Without `keepTrackIds` the tracks kept in place stay. */
  | { type: "rippleDeleteRanges"; ranges: TimeRange[]; keepTrackIds?: string[] | null }
  | { type: "setCanvas"; width: number; height: number; background?: string | null; backgroundBlur?: number | null }
  | { type: "renameProject"; name: string };
