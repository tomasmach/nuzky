import { memo, useEffect } from "react";
import { AudioLines, Film, Gauge, Image as ImageIcon, Type } from "lucide-react";
import { MAIN_TRACK, isCaptionTrack, useEditor } from "../../lib/store";
import { US, formatDuration } from "../../lib/time";
import type { Asset, Clip, Filmstrip, Project, Track } from "../../lib/types";
import { Waveform } from "./Waveform";

export const EDGE = 7;

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
  ghost,
  timing,
  visible,
  onPointerDown,
  onContextMenu,
}: {
  clip: Clip;
  track: Track;
  project: Project;
  zoom: number;
  height: number;
  selected: boolean;
  ghost?: boolean;
  timing?: { startUs: number; durationUs: number };
  /** Visible lane range in px, so long clips only draw the frames on screen. */
  visible: [number, number];
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

  return (
    <div
      data-clip-id={clip.id}
      onPointerDown={onPointerDown ? (e) => onPointerDown(e, clip, track) : undefined}
      onContextMenu={onContextMenu ? (e) => onContextMenu(e, clip) : undefined}
      className={`group absolute top-1 bottom-1 overflow-hidden rounded-md ${bg} ${
        ghost ? "z-30 opacity-85 shadow-xl shadow-black/60 ring-2 ring-accent" : selected ? "z-10 ring-2 ring-accent" : "ring-1 ring-black/40 hover:ring-muted/60"
      } ${track.hidden ? "opacity-40" : ""} cursor-grab active:cursor-grabbing`}
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
        <Waveform assetId={asset.id} sourceInUs={c.sourceInUs} durationUs={durationUs} speed={speed} width={width} color="#5fd3a5" />
      )}
      {soundStrip && (
        <div className="pointer-events-none absolute inset-x-0 bottom-0 h-3.5 bg-black/45">
          <Waveform assetId={asset.id} sourceInUs={c.sourceInUs} durationUs={durationUs} speed={speed} width={width} color="#8fe3c1" className="absolute inset-0 h-full" />
        </div>
      )}

      <div className="pointer-events-none absolute inset-x-0 top-0 flex items-center gap-1 px-2.5 pt-1 text-[11px] text-fg">
        <span className={`flex min-w-0 items-center gap-1 rounded-sm px-1 ${visual ? "bg-black/60" : ""}`}>
          {/* Caption clips are short; the track header carries their icon, so the text gets the room. */}
          {!isCaption && <Icon size={11} className="shrink-0" />}
          <span className="truncate">{label}</span>
          {width > 110 && <span className="tabular shrink-0 pl-1 text-fg/80">{formatDuration(durationUs)}</span>}
        </span>
        {speed !== 1 && width > 40 && (
          <span className="tabular flex shrink-0 items-center gap-0.5 rounded-sm bg-black/60 px-1 font-medium">
            <Gauge size={11} />
            {Number(speed.toFixed(2))}x
          </span>
        )}
      </div>

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
              onPointerDown={(e) => {
                e.stopPropagation();
                useEditor.getState().seek(clip.startUs + k.tUs);
              }}
              className="absolute -top-[5px] h-[10px] w-[10px] -translate-x-1/2 rotate-45 rounded-[1px] border border-black/70 bg-fg hover:bg-accent"
              // Clear of the trim handles, but never so far in that a short clip's first and last keys meet.
              style={{ left: Math.max(Math.min(EDGE + 7, width / 4), Math.min(width - Math.min(EDGE + 7, width / 4), (k.tUs / US) * zoom)) }}
            />
          ))}
        </div>
      )}

      {/* Trim handles: visible on hover and selection so edges look grabbable. */}
      <div className={`absolute inset-y-0 left-0 w-[7px] cursor-ew-resize rounded-l-md ${selected ? "bg-white/90" : "group-hover:bg-white/50"}`} />
      <div className={`absolute inset-y-0 right-0 w-[7px] cursor-ew-resize rounded-r-md ${selected ? "bg-white/90" : "group-hover:bg-white/50"}`} />
    </div>
  );
});
