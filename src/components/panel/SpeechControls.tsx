import { useEffect, useState } from "react";
import { AlertCircle, Loader2, X } from "lucide-react";
import { api, errorText, plainError } from "../../lib/api";
import { SPEECH_LANGUAGES, setSpeechLanguage, useSpeech } from "../../lib/speech";
import { useEditor } from "../../lib/store";
import type { JobEvent, SpeechModel } from "../../lib/types";
import { Button, Field, ProgressBar } from "../ui";

export const selectClass = "h-8 rounded-md border border-line bg-raised px-2 text-[13px] text-fg disabled:cursor-not-allowed disabled:opacity-40";

const isSpeech = (j: JobEvent) => j.kind === "captions" || j.kind === "transcript";

/** The running recognition job (captions and transcripts share one slot) and the last one of `kind`. */
export function useSpeechJobs(kind: JobEvent["kind"]) {
  const jobs = useEditor((s) => s.jobs);
  const all = Object.values(jobs);
  return { running: all.find((j) => isSpeech(j) && j.status === "running"), last: all.filter((j) => j.kind === kind).pop() };
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
        <select value={language} disabled={disabled} onChange={(e) => setSpeechLanguage(e.target.value)} className={selectClass}>
          {SPEECH_LANGUAGES.map(([id, name]) => (
            <option key={id} value={id}>
              {name}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Accuracy">
        {modelsError && models.length === 0 ? (
          <span className="flex gap-1.5 text-[12px] text-danger" role="alert">
            <AlertCircle size={14} className="mt-px shrink-0" />
            Could not load the models: {modelsError}
          </span>
        ) : (
          <select value={model} disabled={disabled} onChange={(e) => useSpeech.setState({ model: e.target.value })} className={selectClass}>
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.label} ({m.sizeMb} MB){m.downloaded ? "" : " · download"}
              </option>
            ))}
          </select>
        )}
      </Field>
    </>
  );
}

/** Progress of a running recognition job, with Cancel. */
export function SpeechJobCard({ job }: { job: JobEvent }) {
  // No fill, so the empty part of the progress track stays visible.
  return (
    <div className="flex flex-col gap-2 rounded-md border border-line py-2 pl-3 pr-1.5" role="status">
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
