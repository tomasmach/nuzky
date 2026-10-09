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
  /** Mirrored left to right after the rotation; absent when not. */
  mirror?: boolean;
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
  /** Lines wrap at this width in canvas pixels; captions on vertical videos keep to the Reels/TikTok safe area. */
  maxWidth?: number | null;
  /** Karaoke: `#rrggbb` fill of the word being spoken; null or missing draws every word in `color`. */
  highlight?: string | null;
}

/** A spoken word of a caption; times are relative to the clip start. */
export interface CaptionWord {
  text: string;
  startUs: number;
  endUs: number;
}

/** All 0 = unchanged. -1..1, fade and vignette 0..1. */
export interface Adjust {
  exposure: number;
  tint: number;
  highlights: number;
  shadows: number;
  fade: number;
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
      /** High-pass, gentle denoise and de-ess for speech; playback switches to it once prepared. */
      cleanVoice: boolean;
    }
  | {
      type: "text";
      text: string;
      style: TextStyle;
      transform: Transform;
      /** Generated captions: the spoken words, which highlight only while `text` is them joined by single spaces. */
      words?: CaptionWord[];
    };

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
  /** "reels": Instagram Reels and TikTok, 1080×1920 at 30 fps, sound levelled to −14 LUFS in the file. Needs a 9:16 canvas; resolution and fps must be 1080 and 30. */
  preset?: "reels" | null;
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
  /** Where Reels and TikTok draw nothing over a vertical video, in canvas pixels; computed by the engine, null for other formats. */
  safeArea?: SafeArea | null;
}

export interface SafeArea {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** Editing limits the engine enforces, sent at boot so the UI never copies them. */
export interface Limits {
  minSpeed: number;
  maxSpeed: number;
  maxTransitionUs: number;
  /** Vertical offset of generated captions from the canvas centre, as a fraction of its height. */
  captionY: number;
}

export interface Project {
  version: number;
  name: string;
  canvas: Canvas;
  assets: Asset[];
  tracks: Track[];
  /** Recognised words the user corrected; absent when there are none. */
  wordCorrections?: WordCorrection[];
}

/** A word as the user corrected it; applies while the file's transcript has `original` starting at `sourceStartUs`. */
export interface WordCorrection {
  assetId: string;
  sourceStartUs: number;
  original: string;
  text: string;
}

export interface Snapshot {
  sessionEpoch: string;
  /** Label of the AI run editing the project, or null. */
  openRunLabel: string | null;
  recovery: boolean;
  project: Project;
  revision: number;
  canUndo: boolean;
  canRedo: boolean;
  path: string;
  select: string[];
  /** Why AI agents cannot attach to this project live; editing works without them. */
  agentBridgeError: string | null;
}

export interface Transport {
  playing: boolean;
  tUs: number;
}

export interface Boot {
  snapshot: Snapshot;
  previewUrl: string;
  transport: Transport;
  limits: Limits;
  /** Why the preview could not start, when it failed before the UI listened. */
  engineError: string | null;
  /** Why the most recent project was not opened, when another one opened instead. */
  startupNotice: string | null;
  version: string;
  /** False when update checks are turned off for this installation. */
  updateChecks: boolean;
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

/** A project on the home screen. */
export interface LibraryProject {
  path: string;
  name: string;
  modifiedMs: number;
  durationUs: number;
  /** Canvas size; 0 for a project that cannot be read. */
  width: number;
  height: number;
  collection: string | null;
  /**
   * broken: the file cannot be read. busy: another Nuzky window or an agent has it open.
   * missing: some of its media files are gone. empty: nothing on the timeline.
   */
  state: "broken" | "busy" | "missing" | "empty" | null;
  missing: number;
  /** Why a broken project cannot be read. */
  error: string | null;
}

export interface Collection {
  id: string;
  name: string;
}

export interface Library {
  projects: LibraryProject[];
  collections: Collection[];
  /** Set when the collections could not be read. */
  notice: string | null;
}

/** What Undo needs to bring a deleted collection back. */
export interface DeletedCollection {
  collection: Collection;
  position: number;
  paths: string[];
}

/** Where words of a search were said: the project and its timeline time. */
export interface SaidHit {
  path: string;
  startUs: number;
  before: string;
  text: string;
  after: string;
}

export interface Said {
  hits: SaidHit[];
  /** Projects with speech that is not transcribed, so only their names were searched. */
  untranscribed: string[];
  /** How many projects have a transcript to search. */
  transcribed: number;
}

export interface SpeechModel {
  id: string;
  label: string;
  sizeMb: number;
  downloaded: boolean;
}

/** Families the engine can draw: bundled ones ship with Nuzky, system ones are installed here. */
export interface FontFamilies {
  bundled: string[];
  system: string[];
}

/** A word where it is heard on the timeline; `i` numbers it for cuts. */
export interface TranscriptWord {
  i: number;
  startUs: number;
  endUs: number;
  text: string;
  p: number;
  /** What recognition wrote, when the user corrected the word. */
  original?: string;
}

/** Silence in speech longer than the pause length; removing it cuts `startUs`–`endUs` out of the whole `gapUs`. */
export interface TranscriptPause {
  startUs: number;
  endUs: number;
  gapUs: number;
}

/** The timeline's speech as it is now, from the transcripts of its media files. */
export interface TranscriptView {
  /** Sent back with a cut, which is refused once the speech moved or was recognised again. */
  key: string;
  words: TranscriptWord[];
  pauses: TranscriptPause[];
  /** Heard media without a transcript yet. */
  untranscribed: string[];
}

export interface WordsCorrected {
  snapshot: Snapshot;
  /** Caption clips that now show the corrected words. */
  captions: number;
}

export interface TranscriptCut {
  snapshot: Snapshot;
  /** Where the first cut starts, which is now where what followed it plays. */
  startUs: number;
  removedUs: number;
}

/** A sentence said with emphasis, proposed for a punch-in. `from`/`to` are inclusive word numbers of the transcript view. */
export interface SuggestedZoom {
  from: number;
  to: number;
  startUs: number;
  endUs: number;
  text: string;
  score: number;
  scale: number;
}

export interface ZoomSuggestions {
  /** The transcript view's key the word numbers belong to; applying is refused once it changed. */
  key: string;
  zooms: SuggestedZoom[];
}

export interface ZoomsApplied {
  snapshot: Snapshot;
  /** False when every clip in reach has keyframes, so nothing changed. */
  changed: boolean;
  /** Clips with keyframes the zooms left alone. */
  skipped: number;
}

/** A punch-in over the timeline range [startUs, endUs): the main track's picture scaled by `scale`. */
export interface ZoomRange {
  startUs: number;
  endUs: number;
  scale: number;
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
  /** Timeline times, unlike the words a clip stores. */
  words?: CaptionWord[];
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
      cleanVoice?: boolean | null;
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
  | { type: "renameProject"; name: string }
  /** Text equal to `original` removes the correction. */
  | { type: "correctWords"; corrections: WordCorrection[] }
  /** Splits main-track clips at the range edges and multiplies the scale inside; clips with keyframes stay as they are. */
  | { type: "zoomRanges"; ranges: ZoomRange[] };

/** An agent Nuzky can be connected to, and the state of its `nuzky` MCP entry. */
export type AgentKind = "claudeCode" | "codex";
export interface AgentConnection {
  agent: AgentKind;
  name: string;
  /** Its config file. */
  path: string;
  /** connected: runs this app; other: a nuzky entry that runs something else. */
  state: "connected" | "other" | "missing" | "unreadable";
  problem: string | null;
  /** The copy made before the last change. */
  backup: string | null;
}
