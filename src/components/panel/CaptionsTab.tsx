import { useEffect, useState } from "react";
import { AlertCircle, Captions, Loader2, X } from "lucide-react";
import { api, errorText } from "../../lib/api";
import { CAPTION_STYLES, sameStyle } from "../../lib/presets";
import { applyCaptionStyle, isCaptionTrack, useEditor } from "../../lib/store";
import type { CaptionModel } from "../../lib/types";
import { Button, Field, ProgressBar, TextSwatch } from "../ui";

const LANGUAGES = [
  ["auto", "Detect automatically"],
  ["cs", "Czech"],
  ["sk", "Slovak"],
  ["en", "English"],
  ["de", "German"],
  ["pl", "Polish"],
  ["es", "Spanish"],
  ["fr", "French"],
  ["it", "Italian"],
  ["uk", "Ukrainian"],
];

const selectClass = "h-8 rounded-md border border-line bg-raised px-2 text-[13px] text-fg disabled:cursor-not-allowed disabled:opacity-40";

export function CaptionsTab() {
  const [models, setModels] = useState<CaptionModel[]>([]);
  const [model, setModel] = useState("large-v3-turbo-q5_0");
  const [language, setLanguage] = useState("cs");
  const [styleIdx, setStyleIdx] = useState(0);
  const jobs = useEditor((s) => s.jobs);
  const captionStyle = useEditor((s) => {
    const track = s.snap?.project.tracks.find(isCaptionTrack);
    const first = track?.clips[0]?.content;
    return first?.type === "text" ? first.style : null;
  });
  const job = Object.values(jobs).find((j) => j.kind === "captions" && j.status === "running");
  const lastJob = Object.values(jobs).filter((j) => j.kind === "captions").pop();
  const hasCaptions = captionStyle !== null;
  // With captions on the timeline the highlighted style is the one they use.
  const current = hasCaptions ? CAPTION_STYLES.findIndex((s) => sameStyle(s.style, captionStyle)) : styleIdx;

  useEffect(() => {
    api.captionModels().then(setModels);
  }, [job?.id, lastJob?.status]);

  const start = async () => {
    try {
      await api.startCaptions(model, language, CAPTION_STYLES[current >= 0 ? current : styleIdx].style);
    } catch (e) {
      useEditor.getState().toast({ kind: "error", text: errorText(e) });
    }
  };
  const pickStyle = (i: number) => {
    setStyleIdx(i);
    if (hasCaptions) applyCaptionStyle(CAPTION_STYLES[i].style);
  };
  const running = !!job;

  return (
    <div className="flex flex-col gap-4 overflow-y-auto p-3">
      <p className="text-[12px] text-muted">Speech is recognised on this computer. Nothing is uploaded.</p>
      <Field label="Spoken language">
        <select value={language} disabled={running} onChange={(e) => setLanguage(e.target.value)} className={selectClass}>
          {LANGUAGES.map(([id, name]) => (
            <option key={id} value={id}>
              {name}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Accuracy">
        <select value={model} disabled={running} onChange={(e) => setModel(e.target.value)} className={selectClass}>
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.label} ({m.sizeMb} MB){m.downloaded ? "" : " · download"}
            </option>
          ))}
        </select>
      </Field>
      <div className="flex flex-col gap-1.5">
        <span className="text-[12px] text-muted">{hasCaptions ? "Style · applies to all captions" : "Style"}</span>
        <div className="grid grid-cols-2 gap-2">
          {CAPTION_STYLES.map((s, i) => (
            <button
              key={s.name}
              type="button"
              aria-pressed={i === current}
              disabled={running}
              title={hasCaptions ? `Apply ${s.name} to all captions` : `Use ${s.name} for new captions`}
              onClick={() => pickStyle(i)}
              className={`flex h-12 items-center justify-center rounded-md border bg-[#2b3036] disabled:cursor-not-allowed disabled:opacity-40 ${
                i === current ? "border-accent shadow-[0_0_0_1px_var(--color-accent)]" : "border-line enabled:hover:border-muted"
              }`}
            >
              <TextSwatch style={s.style} label={s.name} />
            </button>
          ))}
        </div>
      </div>
      {/* The running card has no fill, so the empty part of the progress track stays visible. */}
      {job ? (
        <div className="flex flex-col gap-2 rounded-md border border-line py-2 pl-3 pr-1.5" role="status">
          <div className="flex items-center gap-2 text-[12px]">
            <Loader2 size={13} className="shrink-0 animate-spin text-accent" />
            <span className="min-w-0 flex-1 truncate text-fg">{job.phase ?? "Starting"}</span>
            {job.progress > 0 && <span className="tabular text-muted">{Math.round(job.progress * 100)}%</span>}
            <Button variant="ghost" className="h-7 px-2" onClick={() => api.cancelJob(job.id)}>
              <X size={14} /> Cancel
            </Button>
          </div>
          <ProgressBar value={job.progress} className="mr-1.5" />
        </div>
      ) : (
        <Button variant="primary" onClick={start}>
          <Captions size={15} /> {hasCaptions ? "Regenerate captions" : "Generate captions"}
        </Button>
      )}
      {!job && hasCaptions && <p className="-mt-2 text-[12px] text-muted">Regenerating replaces the current Captions track.</p>}
      {!job && lastJob?.status === "failed" && (
        <p className="flex gap-1.5 text-[12px] text-danger" role="alert">
          <AlertCircle size={14} className="mt-px shrink-0" />
          {lastJob.message}
        </p>
      )}
    </div>
  );
}
