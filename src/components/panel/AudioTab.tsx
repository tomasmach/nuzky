import { AudioLines, Music, Plus, Trash2 } from "lucide-react";
import { useEditor } from "../../lib/store";
import { formatDuration } from "../../lib/time";
import type { Asset } from "../../lib/types";
import { Waveform } from "../timeline/Waveform";
import { ProgressBar, lockedProps, useLockReason } from "../ui";
import { ImportPlaceholder, ImportTile, addAtPlayhead, assetDragHandler, pickAndImport, removeAsset } from "./assets";

/** Round row action. */
const ACTION = "flex h-7 w-7 items-center justify-center rounded-full transition-colors duration-[120ms] ease-out aria-disabled:cursor-not-allowed aria-disabled:opacity-40";

function AudioItem({ asset }: { asset: Asset }) {
  const job = useEditor((s) => s.jobs[`audio:${asset.id}`]);
  const lock = useLockReason();
  return (
    <div
      onPointerDown={assetDragHandler(asset.id)}
      onDoubleClick={() => addAtPlayhead(asset.id)}
      className="group relative flex cursor-grab items-center gap-2.5 rounded-xl p-1.5 hover:bg-raised active:cursor-grabbing"
      title={`${asset.name}\nDrag onto the timeline, or press + to add at the playhead`}
    >
      <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-md bg-clip-audio text-fg">
        <AudioLines size={18} />
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate text-[13px] text-fg">{asset.name}</span>
        <div className="relative h-3.5">
          {job?.status === "running" ? (
            <ProgressBar value={job.progress} label={job.label} className="mt-1" />
          ) : (
            <Waveform assetId={asset.id} sourceInUs={0} durationUs={asset.durationUs} speed={1} width={180} color="#5fd3a5" className="h-full" />
          )}
        </div>
      </div>
      <span className="tabular shrink-0 text-[12px] text-muted">{formatDuration(asset.durationUs)}</span>
      {/* Shown on hover or focus. */}
      <div className="flex shrink-0 items-center gap-1 opacity-0 group-focus-within:opacity-100 group-hover:opacity-100">
        <button
          type="button"
          aria-label={`Remove ${asset.name}`}
          title="Remove from project, with its clips"
          onClick={() => removeAsset(asset)}
          {...lockedProps(lock)}
          className={`text-muted hover:bg-white/[.08] hover:text-danger aria-disabled:hover:bg-transparent aria-disabled:hover:text-muted ${ACTION}`}
        >
          <Trash2 size={14} />
        </button>
        <button
          type="button"
          aria-label={`Add ${asset.name} at playhead`}
          title="Add at playhead"
          onClick={() => addAtPlayhead(asset.id)}
          {...lockedProps(lock)}
          className={`btn-prominent ${ACTION}`}
        >
          <Plus size={16} />
        </button>
      </div>
    </div>
  );
}

export function AudioTab() {
  const assets = useEditor((s) => s.snap?.project.assets);
  const audio = (assets ?? []).filter((a) => a.kind === "audio");
  // Filtered outside the selector: a new array on every read makes React re-render without end.
  const importing = useEditor((s) => s.importing).filter((i) => i.kind === "audio");
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3.5 pt-3.5">
      <ImportTile label="Add music or sound" onClick={() => pickAndImport(true)} className="mx-3.5" />
      {audio.length === 0 && importing.length === 0 ? (
        <div className="mx-3.5 mb-3.5 flex flex-1 flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-white/[.1] p-6 text-center">
          <Music size={28} className="text-muted" />
          <p className="text-[13px] text-fg">No music yet</p>
          <p className="text-[12px] text-muted">MP3, WAV, M4A, FLAC or OGG files you add show up here.</p>
        </div>
      ) : (
        <div className="flex min-h-0 flex-col gap-0.5 overflow-y-auto px-2 pb-3.5">
          {audio.map((a) => (
            <AudioItem key={a.id} asset={a} />
          ))}
          {importing.map((i) => (
            <ImportPlaceholder key={i.key} name={i.name} compact />
          ))}
        </div>
      )}
    </div>
  );
}
