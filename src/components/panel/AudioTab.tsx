import { AudioLines, Music, Plus, Trash2 } from "lucide-react";
import { type AudioTab as Tab, useSounds } from "../../lib/sounds";
import { useEditor } from "../../lib/store";
import { formatDuration } from "../../lib/time";
import type { Asset } from "../../lib/types";
import { Waveform } from "../timeline/Waveform";
import { ProgressBar, TabBar, TabPanel, lockedProps, useLockReason } from "../ui";
import { ImportPlaceholder, ImportTile, addAtPlayhead, assetDragHandler, pickAndImport, removeAsset } from "./assets";
import { CreditsBar, SoundLibrary } from "./SoundLibrary";

const TABS: { id: Tab; label: string }[] = [
  { id: "project", label: "In project" },
  { id: "music", label: "Music" },
  { id: "effects", label: "Sound effects" },
];

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
      title={`${asset.name}${asset.credit ? `\nBy ${asset.credit.author}, ${asset.credit.license === "by" ? "CC BY" : "CC0"} ${asset.credit.licenseVersion}` : ""}\nDrag onto the timeline, or press + to add at the playhead`}
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
      {asset.credit?.license === "by" && (
        <span title="Needs credit in the video's description" className="shrink-0 rounded-[4px] px-[5px] py-px text-[11px] font-medium text-muted shadow-[inset_0_0_0_1px_rgb(255_255_255/.14)]">
          CC BY
        </span>
      )}
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

/** The project's own sound files, as the Audio panel always showed them. */
function InProject() {
  const assets = useEditor((s) => s.snap?.project.assets);
  const audio = (assets ?? []).filter((a) => a.kind === "audio");
  // Filtered outside the selector: a new array on every read makes React re-render without end.
  const importing = useEditor((s) => s.importing).filter((i) => i.kind === "audio");
  return (
    <>
      <ImportTile label="Add music or sound" onClick={() => pickAndImport(true)} className="mx-3.5" />
      {audio.length === 0 && importing.length === 0 ? (
        <div className="mx-3.5 mb-3.5 flex flex-1 flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-white/[.1] p-6 text-center">
          <Music size={28} className="text-muted" />
          <p className="text-[13px] text-fg">No music yet</p>
          <p className="text-[12px] text-muted">MP3, WAV, M4A, FLAC or OGG files you add show up here, and so do sounds from Music and Sound effects.</p>
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
    </>
  );
}

export function AudioTab() {
  const tab = useSounds((s) => s.tab);
  return (
    <div className="flex min-h-0 flex-1 flex-col pt-3">
      <TabBar group="audio" label="Audio" tabs={TABS} value={tab} onChange={(id) => useSounds.setState({ tab: id })} />
      <TabPanel group="audio" id={tab} className="flex min-h-0 flex-1 flex-col gap-3.5 pt-2.5">
        {tab === "project" && <InProject />}
        {tab === "music" && <SoundLibrary kind="music" />}
        {tab === "effects" && <SoundLibrary kind="effect" />}
      </TabPanel>
      <CreditsBar />
    </div>
  );
}
