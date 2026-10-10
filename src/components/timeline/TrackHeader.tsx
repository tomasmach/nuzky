import { memo, type ReactNode } from "react";
import { AudioLines, Captions, Eye, EyeOff, Film, Type, Volume2, VolumeX } from "lucide-react";
import { MAIN_TRACK, isCaptionTrack, useEditor } from "../../lib/store";
import { CoverTile } from "../cover/CoverTile";
import type { Track } from "../../lib/types";

/** Track header toggle. Off states swap the icon and brighten it; accent stays for selection. */
function TrackToggle({ off, label, onIcon, offIcon, onClick, locked }: { off: boolean; label: string; onIcon: ReactNode; offIcon: ReactNode; onClick: () => void; locked: boolean }) {
  return (
    <button
      type="button"
      aria-label={locked ? `${label}: the AI is editing` : label}
      title={locked ? `${label}: the AI is editing` : label}
      aria-pressed={off}
      // Focusable while locked, so the keyboard reaches it and its reason.
      aria-disabled={locked || undefined}
      onClick={locked ? undefined : onClick}
      className={`inline-flex h-[22px] w-[22px] shrink-0 items-center justify-center rounded-md transition-colors duration-[120ms] ease-out aria-disabled:cursor-not-allowed aria-disabled:opacity-40 ${off ? "bg-raised text-fg" : locked ? "text-muted" : "text-muted hover:bg-white/[.08] hover:text-fg"}`}
    >
      {off ? offIcon : onIcon}
    </button>
  );
}

/**
 * Kind icon, name, and the hide and mute toggles of a track, pinned left while the lanes scroll.
 * It fills its whole row, gap included, and sits above the playhead (z 45), so the header column
 * stays unbroken and nothing scrolled under it shows through.
 */
export const TrackHeader = memo(function TrackHeader({ track, width, locked }: { track: Track; width: number; locked: boolean }) {
  const { edit } = useEditor.getState();
  const KindIcon = track.kind === "audio" ? AudioLines : track.kind === "text" ? (isCaptionTrack(track) ? Captions : Type) : Film;
  return (
    // 22 px toggles leave the name about 50 px, enough for "Captions" and "Overlay".
    <div className="sticky left-0 z-[47] flex shrink-0 items-center gap-0.5 border-r border-white/[.07] bg-panel pl-2 pr-1" style={{ width }}>
      {/* The main track starts with the Cover tile, where CapCut has it; the toggles keep their columns. */}
      {track.id === MAIN_TRACK ? (
        <span className="min-w-0 flex-1">
          <CoverTile />
        </span>
      ) : (
        <>
          <KindIcon size={14} className="mr-1 shrink-0 text-muted" />
          <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-fg">{track.name || track.kind}</span>
        </>
      )}
      {/* Fixed columns: eye, then speaker; a spacer keeps the column when a toggle does not apply. */}
      {track.kind !== "audio" ? (
        <TrackToggle
          off={track.hidden}
          label={track.hidden ? `Show ${track.name}` : `Hide ${track.name}`}
          onIcon={<Eye size={14} />}
          offIcon={<EyeOff size={14} />}
          onClick={() => edit({ type: "updateTrack", trackId: track.id, hidden: !track.hidden })}
          locked={locked}
        />
      ) : (
        <span className="w-[22px] shrink-0" aria-hidden />
      )}
      {track.kind !== "text" ? (
        <TrackToggle
          off={track.muted}
          label={track.muted ? `Unmute ${track.name}` : `Mute ${track.name}`}
          onIcon={<Volume2 size={14} />}
          offIcon={<VolumeX size={14} />}
          onClick={() => edit({ type: "updateTrack", trackId: track.id, muted: !track.muted })}
          locked={locked}
        />
      ) : (
        <span className="w-[22px] shrink-0" aria-hidden />
      )}
    </div>
  );
});
