import { useEffect, useState } from "react";
import { AudioLines, Film, Plus, Trash2, Upload } from "lucide-react";
import { useEditor } from "../../lib/store";
import { formatDuration } from "../../lib/time";
import type { Asset } from "../../lib/types";
import { Button, ProgressBar } from "../ui";
import { KindIcon, RemoveConfirm, addAtPlayhead, assetDragHandler, pickAndImport } from "./assets";

function MediaItem({ asset }: { asset: Asset }) {
  const thumb = useEditor((s) => s.thumbs[asset.id]);
  const job = useEditor((s) => s.jobs[`audio:${asset.id}`]);
  const [confirm, setConfirm] = useState(false);
  useEffect(() => useEditor.getState().loadThumb(asset.id), [asset.id]);
  const preparing = job?.status === "running";

  return (
    <div
      onPointerDown={assetDragHandler(asset.id)}
      onDoubleClick={() => addAtPlayhead(asset.id)}
      className="group relative flex cursor-grab flex-col gap-1 rounded-md p-1 hover:bg-raised active:cursor-grabbing"
      title={`${asset.name}\nDrag onto the timeline, or press + to add at the playhead`}
    >
      <div className="relative aspect-video overflow-hidden rounded bg-bg">
        {asset.kind === "audio" ? (
          <div className="flex h-full items-center justify-center bg-clip-audio/50 text-muted">
            <AudioLines size={28} />
          </div>
        ) : thumb === undefined ? (
          <div className="skeleton h-full w-full" />
        ) : thumb ? (
          <img src={thumb} alt="" className="h-full w-full object-contain" draggable={false} />
        ) : (
          <div className="flex h-full items-center justify-center text-muted">
            <KindIcon kind={asset.kind} size={24} />
          </div>
        )}
        <span className="tabular absolute bottom-1 right-1 rounded bg-black/75 px-1 text-[11px] text-fg">
          {asset.kind === "image" ? "Image" : formatDuration(asset.durationUs)}
        </span>
        <button
          type="button"
          aria-label={`Add ${asset.name} at playhead`}
          title="Add at playhead"
          onClick={() => addAtPlayhead(asset.id)}
          className="absolute right-1 top-1 flex h-7 w-7 items-center justify-center rounded-md bg-accent text-black opacity-0 shadow transition-opacity duration-[120ms] hover:bg-accent-strong group-hover:opacity-100 group-focus-within:opacity-100"
        >
          <Plus size={16} />
        </button>
        <button
          type="button"
          aria-label={`Remove ${asset.name}`}
          title="Remove from project"
          onClick={() => setConfirm(true)}
          className="absolute left-1 top-1 flex h-7 w-7 items-center justify-center rounded-md bg-black/70 text-fg opacity-0 transition-opacity duration-[120ms] hover:bg-black/90 hover:text-danger group-hover:opacity-100 group-focus-within:opacity-100"
        >
          <Trash2 size={14} />
        </button>
        {preparing && (
          <div className="absolute inset-x-1 bottom-1 mr-12">
            <ProgressBar value={job.progress} label={job.label} />
          </div>
        )}
      </div>
      <div className="flex items-center gap-1 px-0.5 text-[12px] text-muted">
        <KindIcon kind={asset.kind} size={12} />
        <span className="truncate">{asset.name}</span>
      </div>
      {confirm && <RemoveConfirm asset={asset} onKeep={() => setConfirm(false)} />}
    </div>
  );
}

export function MediaTab() {
  const assets = useEditor((s) => s.snap?.project.assets ?? []);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="p-3">
        <Button variant="primary" className="w-full" onClick={() => pickAndImport()}>
          <Upload size={15} /> Import media
        </Button>
      </div>
      {assets.length === 0 ? (
        <div className="m-3 mt-0 flex flex-1 flex-col items-center justify-center gap-2 rounded-lg border border-dashed border-line p-6 text-center">
          <Film size={28} className="text-muted" />
          <p className="text-[13px] text-fg">No media yet</p>
          <p className="text-[12px] text-muted">Import videos, music or images, or drop files anywhere on the window.</p>
        </div>
      ) : (
        <div className="grid min-h-0 grid-cols-2 content-start gap-1 overflow-y-auto px-2 pb-3">
          {assets.map((a) => (
            <MediaItem key={a.id} asset={a} />
          ))}
        </div>
      )}
    </div>
  );
}
