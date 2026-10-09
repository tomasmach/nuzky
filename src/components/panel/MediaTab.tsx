import { useEffect } from "react";
import { AudioLines, Film, PictureInPicture2, Plus, Trash2 } from "lucide-react";
import { useEditor } from "../../lib/store";
import { formatDuration } from "../../lib/time";
import type { Asset } from "../../lib/types";
import { ProgressBar, lockedProps, useLockReason } from "../ui";
import { ImportPlaceholder, ImportTile, KindIcon, addAtPlayhead, addPictureInPicture, assetDragHandler, pickAndImport, removeAsset } from "./assets";

/** Round action on a media tile; no blur, so a grid of them stays cheap to paint. */
const ACTION = "flex h-[26px] w-[26px] items-center justify-center rounded-full transition-colors duration-[120ms] ease-out aria-disabled:cursor-not-allowed aria-disabled:opacity-40";
const QUIET_ACTION = `bg-black/55 text-white shadow-[inset_0_0_0_1px_rgb(255_255_255/.18)] hover:bg-black/75 aria-disabled:hover:bg-black/55 ${ACTION}`;

function MediaItem({ asset }: { asset: Asset }) {
  const thumb = useEditor((s) => s.thumbs[asset.id]);
  const job = useEditor((s) => s.jobs[`audio:${asset.id}`]);
  const epoch = useEditor((s) => s.snap?.sessionEpoch);
  useEffect(() => useEditor.getState().loadThumb(asset.id), [asset.id, asset.path, epoch]);
  const preparing = job?.status === "running";
  const lock = useLockReason();

  return (
    <div
      onPointerDown={assetDragHandler(asset.id)}
      onDoubleClick={() => addAtPlayhead(asset.id)}
      className="group flex min-w-0 cursor-grab flex-col gap-1.5 active:cursor-grabbing"
      title={`${asset.name}\nDrag onto the timeline, or press + to add at the playhead`}
    >
      {/* The hairline is an outline, so it draws over the thumbnail rather than under it. */}
      <div className="relative aspect-square overflow-hidden rounded-[10px] bg-bg outline-1 -outline-offset-1 outline-white/[.08] group-hover:outline-white/[.22]">
        {asset.kind === "audio" ? (
          <div className="flex h-full items-center justify-center bg-clip-audio/50 text-muted">
            <AudioLines size={26} />
          </div>
        ) : thumb === undefined ? (
          <div className="skeleton h-full w-full" />
        ) : thumb ? (
          <img src={thumb} alt="" className="h-full w-full object-cover" draggable={false} />
        ) : (
          <div className="flex h-full items-center justify-center text-muted">
            <KindIcon kind={asset.kind} size={24} />
          </div>
        )}
        {preparing && (
          <div className="absolute inset-x-1.5 top-1.5">
            <ProgressBar value={job.progress} label={job.label} />
          </div>
        )}
        <span className="tabular absolute right-1 bottom-1 rounded-[4px] bg-black/60 px-1 text-[11px] leading-[15px] font-semibold text-white group-focus-within:opacity-0 group-hover:opacity-0">
          {asset.kind === "image" ? "Image" : formatDuration(asset.durationUs)}
        </span>
        <div className="absolute inset-x-0 bottom-0 flex items-end gap-1.5 bg-linear-to-b from-transparent to-black/45 p-1.5 pt-6 opacity-0 group-focus-within:opacity-100 group-hover:opacity-100">
          <button
            type="button"
            aria-label={`Add ${asset.name} at playhead`}
            title="Add at playhead"
            onClick={() => addAtPlayhead(asset.id)}
            {...lockedProps(lock)}
            className={`btn-prominent ${ACTION}`}
          >
            <Plus size={15} />
          </button>
          {asset.kind !== "audio" && (
            <button
              type="button"
              aria-label={`Add ${asset.name} as picture in picture`}
              title="Picture in picture at the playhead"
              onClick={() => addPictureInPicture(asset.id)}
              {...lockedProps(lock)}
              className={QUIET_ACTION}
            >
              <PictureInPicture2 size={14} />
            </button>
          )}
          <button
            type="button"
            aria-label={`Remove ${asset.name}`}
            title="Remove from project, with its clips"
            onClick={() => removeAsset(asset)}
            {...lockedProps(lock)}
            className={`hover:text-danger aria-disabled:hover:text-white ${QUIET_ACTION}`}
          >
            <Trash2 size={14} />
          </button>
        </div>
      </div>
      <span className="truncate text-[11px] leading-[13px] text-muted">{asset.name}</span>
    </div>
  );
}

export function MediaTab() {
  const assets = useEditor((s) => s.snap?.project.assets ?? []);
  const importing = useEditor((s) => s.importing);
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3.5 pt-3.5">
      {assets.length === 0 && importing.length === 0 ? (
        // Empty: one invitation in the middle, rather than an import tile above a second frame saying the same.
        <div className="mx-3.5 mb-3.5 flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center">
          <Film size={28} className="text-muted" />
          <p className="text-[13px] font-medium text-fg">No media yet</p>
          <p className="max-w-[240px] text-[12px] text-muted">Import videos, music or images, or drop files anywhere on the window.</p>
          <ImportTile label="Import media" hint="Ctrl+I" shortcut="Control+I" onClick={() => pickAndImport()} className="mt-3 w-full max-w-[240px]" />
        </div>
      ) : (
        <>
          <ImportTile label="Import media" hint="Ctrl+I" shortcut="Control+I" onClick={() => pickAndImport()} className="mx-3.5" />
          <div className="grid min-h-0 grid-cols-3 content-start gap-x-2 gap-y-3 overflow-y-auto px-3.5 pb-3.5">
            {assets.map((a) => (
              <MediaItem key={a.id} asset={a} />
            ))}
            {importing.map((i) => (
              <ImportPlaceholder key={i.key} name={i.name} />
            ))}
          </div>
        </>
      )}
    </div>
  );
}
