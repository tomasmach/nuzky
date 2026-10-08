import { useShallow } from "zustand/react/shallow";
import { useEffect, useState, type SelectHTMLAttributes } from "react";
import { AlertCircle, ChevronDown, Loader2, X } from "lucide-react";
import { api, errorText, plainError } from "../../lib/api";
import { SPEECH_LANGUAGES, setSpeechLanguage, useSpeech } from "../../lib/speech";
import { useEditor } from "../../lib/store";
import type { JobEvent, SpeechModel } from "../../lib/types";
import { Button, Field, ProgressBar } from "../ui";

/** A native select drawn as a white 9 % field with a chevron, like the font picker. */
function Select(props: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <span className="relative flex has-[:disabled]:opacity-40">
      <select
        {...props}
        className="h-8 min-w-0 flex-1 appearance-none rounded-lg bg-white/[.09] pr-8 pl-2.5 text-[13px] text-fg shadow-[inset_0_0_0_1px_rgb(255_255_255/.06),inset_0_1px_0_rgb(255_255_255/.06)] transition-colors duration-[120ms] ease-out enabled:hover:bg-white/[.13] disabled:cursor-not-allowed"
      />
      <ChevronDown size={14} aria-hidden className="pointer-events-none absolute top-1/2 right-2.5 -translate-y-1/2 text-muted" />
    </span>
  );
}

const isSpeech = (j: JobEvent) => j.kind === "captions" || j.kind === "transcript";

/**
 * The running recognition job (captions and transcripts share one slot) and the last one of `kind`. Only
 * these two, so the progress of an export or another job does not re-render the tab using them.
 */
export function useSpeechJobs(kind: JobEvent["kind"]) {
  return useEditor(
    useShallow((s) => {
      const all = Object.values(s.jobs);
      return { running: all.find((j) => isSpeech(j) && j.status === "running"), last: all.filter((j) => j.kind === kind).pop() };
    }),
  );
}

/** Spoken language and model, shared by Captions and Transcript. */
export function SpeechFields({ disabled }: { disabled: boolean }) {
  const model = useSpeech((s) => s.model);
  const language = useSpeech((s) => s.language);
  const [models, setModels] = useState<SpeechModel[]>([]);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const { running } = useSpeechJobs("captions");
  // Refreshed when a job starts and ends, so a model downloaded by it loses its "download" note.
  useEffect(() => {
    api.speechModels().then(
      (list) => {
        setModels(list);
        setModelsError(null);
      },
      (e) => setModelsError(plainError(errorText(e))),
    );
  }, [running?.id]);
  return (
    <>
      <Field label="Spoken language">
        <Select value={language} disabled={disabled} onChange={(e) => setSpeechLanguage(e.target.value)}>
          {SPEECH_LANGUAGES.map(([id, name]) => (
            <option key={id} value={id}>
              {name}
            </option>
          ))}
        </Select>
      </Field>
      <Field label="Accuracy">
        {modelsError && models.length === 0 ? (
          <span className="flex gap-1.5 text-[12px] text-danger" role="alert">
            <AlertCircle size={14} className="mt-px shrink-0" />
            Could not load the models: {modelsError}
          </span>
        ) : (
          <Select value={model} disabled={disabled} onChange={(e) => useSpeech.setState({ model: e.target.value })}>
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.label} ({m.sizeMb} MB){m.downloaded ? "" : " · download"}
              </option>
            ))}
          </Select>
        )}
      </Field>
    </>
  );
}

/** Progress of a running recognition job, with Cancel. */
export function SpeechJobCard({ job }: { job: JobEvent }) {
  // No fill, so the empty part of the progress track stays visible.
  return (
    <div className="flex flex-col gap-2 rounded-[10px] border border-white/[.08] py-2 pr-1.5 pl-3" role="status">
      <div className="flex items-center gap-2 text-[12px]">
        <Loader2 size={13} className="shrink-0 animate-spin text-accent" />
        <span className="min-w-0 flex-1 truncate text-fg">
          {job.label} · {job.phase ?? "Starting"}
        </span>
        {job.progress > 0 && <span className="tabular text-muted">{Math.round(job.progress * 100)}%</span>}
        <Button variant="ghost" className="h-7 px-2" onClick={() => api.cancelJob(job.id)}>
          <X size={14} /> Cancel
        </Button>
      </div>
      <ProgressBar value={job.progress} label={job.label} className="mr-1.5" />
    </div>
  );
}

export function JobError({ job }: { job: JobEvent | undefined }) {
  if (job?.status !== "failed") return null;
  return (
    <p className="flex gap-1.5 text-[12px] text-danger" role="alert">
      <AlertCircle size={14} className="mt-px shrink-0" />
      {plainError(job.message ?? "")}
    </p>
  );
}
