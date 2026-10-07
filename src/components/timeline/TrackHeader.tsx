import { memo, type ReactNode } from "react";
import { AudioLines, Captions, Eye, EyeOff, Film, Type, Volume2, VolumeX } from "lucide-react";
import { MAIN_TRACK, isCaptionTrack, useEditor } from "../../lib/store";
import type { Track } from "../../lib/types";

/** Track header toggle. Off states swap the icon and brighten it; accent stays for selection. */
function TrackToggle({ off, label, onIcon, offIcon, onClick }: { off: boolean; label: string; onIcon: ReactNode; offIcon: ReactNode; onClick: () => void }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={off}
      onClick={onClick}
      className={`inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md transition-colors duration-[120ms] ease-out ${off ? "bg-raised text-fg" : "text-muted hover:bg-raised hover:text-fg"}`}
    >
      {off ? offIcon : onIcon}
    </button>
  );
}

/** Kind icon, name, and the hide and mute toggles of a track, pinned left while the lanes scroll. */
export const TrackHeader = memo(function TrackHeader({ track, width }: { track: Track; width: number }) {
  const { edit } = useEditor.getState();
  const isMain = track.id === MAIN_TRACK;
  const KindIcon = track.kind === "audio" ? AudioLines : track.kind === "text" ? (isCaptionTrack(track) ? Captions : Type) : Film;
  return (
    <div className="sticky left-0 z-20 flex shrink-0 items-center gap-1 border-r border-line bg-panel pl-2 pr-1" style={{ width }}>
      <KindIcon size={13} className="shrink-0 text-muted" />
      <span className={`flex-1 truncate text-[12px] ${isMain ? "font-medium text-fg" : "text-muted"}`}>{track.name || track.kind}</span>
      {/* Fixed columns: eye, then speaker; a spacer keeps the column when a toggle does not apply. */}
      {track.kind !== "audio" ? (
        <TrackToggle
          off={track.hidden}
          label={track.hidden ? `Show ${track.name}` : `Hide ${track.name}`}
          onIcon={<Eye size={14} />}
          offIcon={<EyeOff size={14} />}
          onClick={() => edit({ type: "updateTrack", trackId: track.id, hidden: !track.hidden })}
        />
      ) : (
        <span className="w-7 shrink-0" aria-hidden />
      )}
      {track.kind !== "text" ? (
        <TrackToggle
          off={track.muted}
          label={track.muted ? `Unmute ${track.name}` : `Mute ${track.name}`}
          onIcon={<Volume2 size={14} />}
          offIcon={<VolumeX size={14} />}
          onClick={() => edit({ type: "updateTrack", trackId: track.id, muted: !track.muted })}
        />
      ) : (
        <span className="w-7 shrink-0" aria-hidden />
      )}
    </div>
  );
});
