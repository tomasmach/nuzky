import { memo, useEffect } from "react";
import { AudioLines, Film, Gauge, Image as ImageIcon, Type } from "lucide-react";
import { MAIN_TRACK, isCaptionTrack, useEditor } from "../../lib/store";
import { US, formatDuration } from "../../lib/time";
import type { Asset, Clip, Filmstrip, Project, Track } from "../../lib/types";
import { Waveform } from "./Waveform";

/** Width of the trim handles, and of the zone at each end that trims instead of moving. */
export const EDGE = 8;
/** Size of the zone at each top corner of a sound clip that drags its fade instead of trimming. */
const FADE_HANDLE = 16;

export function clipAsset(project: Project, clip: Clip): Asset | undefined {
  const c = clip.content;
  return c.type === "media" ? project.assets.find((a) => a.id === c.assetId) : undefined;
}

/** Frames from the filmstrip sprite matching the source time under each tile. */
function FilmstripTiles({
  strip,
  sourceInUs,
  speed,
  zoom,
  width,
  height,
  visible,
}: {
  strip: Filmstrip;
  sourceInUs: number;
  speed: number;
  zoom: number;
  width: number;
  height: number;
  visible: [number, number];
}) {
  const tileW = Math.max(8, (height * strip.frameWidth) / strip.frameHeight);
  const first = Math.max(0, Math.floor(visible[0] / tileW));
  const last = Math.min(Math.ceil(width / tileW), Math.ceil(visible[1] / tileW));
  const tiles = [];
  for (let i = first; i < last; i++) {
    const sourceUs = sourceInUs + (((i + 0.5) * tileW) / zoom) * US * speed;
    const frame = Math.max(0, Math.min(strip.count - 1, Math.round(sourceUs / strip.intervalUs)));
    tiles.push(
      <div
        key={i}
        className="absolute inset-y-0"
        style={{
          left: i * tileW,
          width: tileW,
          backgroundImage: `url(${strip.url})`,
          backgroundSize: `${strip.count * tileW}px ${height}px`,
          backgroundPosition: `${-frame * tileW}px 0`,
        }}
      />,
    );
  }
  return <div className="pointer-events-none absolute inset-0 overflow-hidden">{tiles}</div>;
}

/** Memoized: playback and pointer moves elsewhere never re-render a clip whose props stay the same. */
export const ClipView = memo(function ClipView({
  clip,
  track,
  project,
  zoom,
  height,
  selected,
  locked,
  tabbable,
  ghost,
  timing,
  visible,
  fades,
  onPointerDown,
  onContextMenu,
}: {
  clip: Clip;
  track: Track;
  project: Project;
  zoom: number;
  height: number;
  selected: boolean;
  /** The AI is editing: the clip can be selected but not moved or trimmed. */
  locked?: boolean;
  /** The timeline's one tab stop among the clips (roving tabindex); arrows move from it. */
  tabbable?: boolean;
  ghost?: boolean;
  timing?: { startUs: number; durationUs: number };
  /** Visible lane range in px, so long clips only draw the frames and waveform on screen. */
  visible: [number, number];
  /** While a fade handle is dragged, the fades it would set. */
  fades?: { inUs: number; outUs: number };
  onPointerDown?: (e: React.PointerEvent, clip: Clip, track: Track) => void;
  onContextMenu?: (e: React.MouseEvent, clip: Clip) => void;
}) {
  const asset = clipAsset(project, clip);
  // Detached sound keeps pointing at the video file; the audio track decides how it looks.
  const sound = track.kind === "audio" || asset?.kind === "audio";
  const visual = !!asset && !sound;
  const thumb = useEditor((s) => (visual ? s.thumbs[asset.id] : undefined));
  const strip = useEditor((s) => (visual && asset.kind === "video" ? s.filmstrips[asset.id] : undefined));
  useEffect(() => {
    if (!visual) return;
    useEditor.getState().loadThumb(asset.id);
    if (asset.kind === "video") useEditor.getState().loadFilmstrip(asset.id);
  }, [asset, visual]);

  const { startUs, durationUs } = timing ?? clip;
  const c = clip.content;
  // Inset by 1 px on each side so the cut between neighbouring clips stays visible.
  const left = (startUs / US) * zoom + 1;
  const width = Math.max(2, (durationUs / US) * zoom - 2);
  const innerH = height - 8;
  const isCaption = c.type === "text" && isCaptionTrack(track);
  const bg = c.type === "text" ? (isCaption ? "bg-clip-captions" : "bg-clip-title") : sound ? "bg-clip-audio" : asset?.kind === "image" ? "bg-clip-image" : "bg-clip-video";
  const Icon = c.type === "text" ? Type : sound ? AudioLines : asset?.kind === "image" ? ImageIcon : Film;
  const label = c.type === "text" ? c.text : (asset?.name ?? "Missing media");
  const speed = c.type === "media" ? c.speed : 1;
  const soundStrip = visual && c.type === "media" && asset.kind === "video" && asset.hasAudio && c.volume > 0;
  const local: [number, number] = [visible[0] - left, visible[1] - left];
  const fade = sound && c.type === "media" ? (fades ?? { inUs: c.fadeInUs, outUs: c.fadeOutUs }) : null;
  const fadeIn = fade ? (fade.inUs / US) * zoom : 0;
  const fadeOut = fade ? (fade.outUs / US) * zoom : 0;
  const fadeHandles = !!fade && !locked && !ghost && width >= 3 * FADE_HANDLE;

  return (
    <div
      data-clip-id={ghost ? undefined : clip.id}
      role={ghost ? undefined : "option"}
      aria-selected={ghost ? undefined : selected}
      tabIndex={ghost ? undefined : tabbable ? 0 : -1}
      onPointerDown={onPointerDown ? (e) => onPointerDown(e, clip, track) : undefined}
      onContextMenu={onContextMenu ? (e) => onContextMenu(e, clip) : undefined}
      // No blur or large shadow here: there is one of these per clip. Only the dragged copy casts a small shadow.
      className={`group absolute top-1 bottom-1 overflow-hidden rounded-[7px] ${bg} ${ghost ? "z-30 opacity-85 shadow-[0_4px_12px_rgb(0_0_0/.5)]" : selected ? "z-10" : ""} ${
        track.hidden ? "opacity-40" : ""
      } ${locked ? "cursor-default" : "cursor-grab active:cursor-grabbing"}`}
      style={{
        left,
        width,
        backgroundImage: visual && !strip && thumb ? `url(${thumb})` : undefined,
        backgroundSize: "auto 100%",
        backgroundRepeat: "repeat-x",
      }}
      aria-label={`${label}, ${formatDuration(durationUs)}`}
    >
      {strip && c.type === "media" && <FilmstripTiles strip={strip} sourceInUs={c.sourceInUs} speed={speed} zoom={zoom} width={width} height={innerH} visible={local} />}
      {sound && asset && c.type === "media" && (
        <Waveform assetId={asset.id} sourceInUs={c.sourceInUs} durationUs={durationUs} speed={speed} width={width} visible={local} color="#30d158" />
      )}
      {(fadeIn > 0 || fadeOut > 0) && (
        // The fades as ramps over the waveform: the shaded corner above the line is how much quieter the sound plays there.
        // The line has a dark edge, like the snap guide, so it stays visible over loud sound.
        <svg className="pointer-events-none absolute left-0 top-0" width={width} height={innerH}>
          {fadeIn > 0 && <path d={`M0 0H${fadeIn}L0 ${innerH}Z`} className="fill-black/45" />}
          {fadeOut > 0 && <path d={`M${width} 0H${width - fadeOut}L${width} ${innerH}Z`} className="fill-black/45" />}
          {[fadeIn > 0 && `M0 ${innerH}L${fadeIn} 0`, fadeOut > 0 && `M${width - fadeOut} 0L${width} ${innerH}`].map(
            (d) =>
              d && (
                <g key={d}>
                  <path d={d} strokeWidth={3} className="stroke-black/50" />
                  <path d={d} strokeWidth={1.5} className="stroke-white/90" />
                </g>
              ),
          )}
        </svg>
      )}
      {soundStrip && (
        <div className="pointer-events-none absolute inset-x-0 bottom-0 h-3.5 bg-black/55">
          <Waveform assetId={asset.id} sourceInUs={c.sourceInUs} durationUs={durationUs} speed={speed} width={width} visible={local} color="#7ee2a8" className="absolute inset-y-0 h-full" />
        </div>
      )}

      {c.type === "text" ? (
        // Text sits straight on its colour, centred; caption clips are short and the track header carries their icon, so the text gets the room.
        <div className={`pointer-events-none absolute inset-0 flex items-center gap-1.5 px-2 text-[11px] text-fg ${isCaption ? "" : "font-medium"}`}>
          {!isCaption && <Icon size={11} className="shrink-0" />}
          <span className="truncate">{label}</span>
          {width > 110 && <span className="tabular shrink-0 font-normal text-fg/70">{formatDuration(durationUs)}</span>}
        </div>
      ) : (
        // Over pictures and waveforms the name sits in a dark chip, readable over bright footage. A plain fill, not a blur: there is one per clip.
        // On sound it sits at the bottom, clear of the fade dots in the top corners.
        <div className={`pointer-events-none absolute inset-x-0 ${sound ? "bottom-1" : "top-1"} flex items-center gap-1 px-2.5 text-[11px] font-medium text-fg`}>
          <span className="flex h-[17px] min-w-0 items-center gap-1 rounded-[5px] bg-black/60 pl-[5px] pr-1.5">
            <Icon size={11} className="shrink-0" />
            <span className="truncate">{label}</span>
            {width > 110 && <span className="tabular shrink-0 pl-0.5 font-normal text-fg/80">{formatDuration(durationUs)}</span>}
          </span>
          {speed !== 1 && width > 40 && (
            <span className="tabular ml-auto flex h-[17px] shrink-0 items-center gap-0.5 rounded-[5px] bg-black/60 px-1 font-semibold">
              <Gauge size={11} />
              {Number(speed.toFixed(2))}x
            </span>
          )}
        </div>
      )}

      {/* The edge goes over the frames: a hairline, or the selection ring. */}
      <span className={`pointer-events-none absolute inset-0 rounded-[7px] ${selected ? "inset-ring-2 inset-ring-accent" : "inset-ring inset-ring-white/10 group-hover:inset-ring-white/30"}`} />

      {clip.animIn && (
        <div
          className="pointer-events-none absolute left-0 top-0 h-[3px] rounded-br bg-fg/80"
          style={{ width: Math.min(width / 2, (clip.animIn.durationUs / US) * zoom) }}
        />
      )}
      {clip.animOut && (
        <div
          className="pointer-events-none absolute right-0 top-0 h-[3px] rounded-bl bg-fg/80"
          style={{ width: Math.min(width / 2, (clip.animOut.durationUs / US) * zoom) }}
        />
      )}

      {/* The main track's cut markers sit at mid-height, so its diamonds run along the bottom. All stay clear of the trim handles. */}
      {selected && !ghost && clip.keyframes.length > 0 && (
        <div className={`absolute inset-x-0 ${track.id === MAIN_TRACK ? "bottom-[7px]" : "top-1/2"}`}>
          {clip.keyframes.map((k) => (
            <button
              key={k.tUs}
              type="button"
              aria-label={`Go to keyframe at ${formatDuration(k.tUs)}`}
              title="Keyframe"
              // Not a clip drag; Enter and Space activate it like a click.
              onPointerDown={(e) => e.stopPropagation()}
              onClick={() => useEditor.getState().seek(clip.startUs + k.tUs)}
              className="absolute -top-[5px] h-[10px] w-[10px] -translate-x-1/2 rotate-45 rounded-[1px] border border-black/70 bg-fg hover:bg-accent"
              // Clear of the trim handles, but never so far in that a short clip's first and last keys meet.
              style={{ left: Math.max(Math.min(EDGE + 7, width / 4), Math.min(width - Math.min(EDGE + 7, width / 4), (k.tUs / US) * zoom)) }}
            />
          ))}
        </div>
      )}

      {/* Trim handles: accent with a dark grip when selected, a faint hint on hover so edges look grabbable. The clip's rounding clips them. */}
      {!locked &&
        (["left-0", "right-0"] as const).map((side) => (
          <div
            key={side}
            className={`absolute inset-y-0 ${side} flex cursor-ew-resize items-center justify-center ${selected ? "bg-accent" : "group-hover:bg-white/25"}`}
            style={{ width: EDGE }}
          >
            {selected && <span className="h-3.5 w-0.5 rounded-full bg-black/55" />}
          </div>
        ))}

      {/* Fade handles: a dot at the end of each fade, over the top of the trim handles while there is none; drag along the
          clip. Each keeps to its half, so two fades meeting in the middle can both still be grabbed. */}
      {fadeHandles &&
        (
          [
            ["in", fadeIn, 0, width / 2 - FADE_HANDLE, "Drag to fade the sound in"],
            ["out", width - fadeOut, width / 2, width - FADE_HANDLE, "Drag to fade the sound out"],
          ] as const
        ).map(([end, x, min, max, title]) => (
          <div
            key={end}
            data-fade={end}
            title={title}
            className={`absolute top-0 flex cursor-ew-resize items-center justify-center ${selected || fades ? "" : "opacity-0 group-hover:opacity-100"}`}
            style={{ left: Math.max(min, Math.min(max, x - FADE_HANDLE / 2)), width: FADE_HANDLE, height: FADE_HANDLE }}
          >
            <span className="h-2.5 w-2.5 rounded-full border border-black/70 bg-fg" />
          </div>
        ))}
    </div>
  );
});
