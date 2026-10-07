import { useMemo } from "react";
import { AlertCircle, AlertTriangle, RefreshCw, ScrollText, Scissors } from "lucide-react";
import { correctWord, cutWords, removePauses, speechBlocker, startSpeech, tokenize, useSpeech, useTranscriptView, type Token } from "../../lib/speech";
import { useEditor } from "../../lib/store";
import { US, formatDuration } from "../../lib/time";
import type { Clip, Project } from "../../lib/types";
import { Button, Checkbox, IconButton, NumberInput } from "../ui";
import { JobError, SpeechFields, SpeechJobCard, useSpeechJobs } from "./SpeechControls";
import { TranscriptText } from "./TranscriptText";

/** Audio tracks with a checkbox: checked ones are not cut, so music keeps playing across cuts. */
function KeepTracks({ project }: { project: Project }) {
  const tracks = project.tracks.filter((t) => t.kind === "audio");
  if (tracks.length === 0) return null;
  const names = (clips: Clip[]) => {
    const ids = new Set(clips.flatMap((c) => (c.content.type === "media" ? [c.content.assetId] : [])));
    return project.assets.filter((a) => ids.has(a.id)).map((a) => a.name).join(", ");
  };
  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-[12px] text-muted">Keep in place while cutting</span>
      {tracks.map((t) => (
        <Checkbox
          key={t.id}
          label={names(t.clips) || t.name}
          checked={t.keepInPlace}
          title="Checked: this track is not cut and keeps playing across cuts. Unchecked: it is cut together with the video."
          onChange={(v) => useEditor.getState().edit({ type: "updateTrack", trackId: t.id, keepInPlace: v })}
        />
      ))}
    </div>
  );
}

/** Clips on the timeline that play the sound of media without a transcript. */
function missingClips(project: Project, untranscribed: string[]): number {
  return project.tracks
    .filter((t) => !t.muted)
    .flatMap((t) => t.clips)
    .filter((c) => c.content.type === "media" && c.content.volume > 0 && untranscribed.includes(c.content.assetId)).length;
}

/** "3 words (1.2 s)", or pauses when no word is included. */
const describe = (tokens: Token[]) => (removedUs: number) => {
  const words = tokens.filter((t) => t.kind === "word").length;
  const n = words || tokens.length;
  return `${n} ${words ? "word" : "pause"}${n === 1 ? "" : "s"} (${formatDuration(removedUs)})`;
};

const total = (tokens: Token[]) => tokens.reduce((s, t) => s + t.endUs - t.startUs, 0);

/** Text-based editing: the timeline's speech as text; deleting words cuts the video. */
export function TranscriptTab() {
  const project = useEditor((s) => s.snap!.project);
  const pauseUs = useSpeech((s) => s.pauseUs);
  const { view, error } = useTranscriptView();
  const { running, last } = useSpeechJobs("transcript");
  const blocker = speechBlocker(project);
  const tokens = useMemo(() => (view ? tokenize(view) : []), [view]);
  const pauses = tokens.filter((t) => t.kind === "pause");
  const missing = view ? missingClips(project, view.untranscribed) : 0;
  const cutBlocker = missing > 0 ? "Transcribe the remaining clips first" : null;
  const busy = !!running;

  const removeAll = () => view && removePauses(view.key, pauseUs, null, describe(pauses));
  const deleteRange = async (lo: number, hi: number) => {
    if (!view) return false;
    const picked = tokens.slice(lo, hi + 1);
    const words = picked.filter((t) => t.kind === "word");
    const done = words.length
      ? await cutWords(view.key, [[words[0].i, words[words.length - 1].i]], describe(picked))
      : await removePauses(view.key, pauseUs, picked.map((t) => t.i), describe(picked));
    // The playhead lands on the cut, where what followed the deleted words now starts.
    if (done) useEditor.getState().seek(done.startUs);
    return !!done;
  };

  const errorNote = error && (
    <p className="flex gap-1.5 text-[12px] text-danger" role="alert">
      <AlertCircle size={14} className="mt-px shrink-0" />
      {error}
    </p>
  );

  // Fetching takes a moment; showing nothing until then keeps the intro from flashing.
  if (!view && !error) return null;
  if (!view || view.words.length === 0)
    return (
      <div className="flex flex-col gap-4 overflow-y-auto p-3">
        <p className="text-[12px] text-muted">Transcribe the timeline, then cut the video by deleting words.</p>
        <SpeechFields disabled={busy} />
        {running ? (
          <SpeechJobCard job={running} />
        ) : (
          <span className="flex">
            {/* With every clip transcribed but no words, transcribing again may use another language. */}
            <Button variant="primary" className="flex-1" disabled={!!blocker || !view} disabledReason={blocker ?? undefined} onClick={() => startSpeech(null, view?.untranscribed.length === 0)}>
              <ScrollText size={15} /> Transcribe timeline
            </Button>
          </span>
        )}
        {!running && <JobError job={last} />}
        {errorNote}
      </div>
    );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-col gap-3 border-b border-line p-3">
        <div className="flex items-center gap-2">
          <span className="tabular flex-1 text-[12px] text-muted">{view.words.length} word{view.words.length === 1 ? "" : "s"}</span>
          <IconButton label={blocker ?? (busy ? "Wait for speech recognition to finish" : "Transcribe again")} className="h-7 w-7" disabled={!!blocker || busy} onClick={() => startSpeech(null, true)}>
            <RefreshCw size={14} />
          </IconButton>
        </div>
        {missing > 0 && (
          <div className="flex items-center gap-1.5 text-[12px] text-fg" role="status">
            <AlertTriangle size={14} className="shrink-0 text-warn" />
            <span className="flex-1">
              {missing} clip{missing === 1 ? "" : "s"} not transcribed
            </span>
            <Button className="h-7 px-2" disabled={busy} disabledReason="Wait for speech recognition to finish" onClick={() => startSpeech(null)}>
              Transcribe
            </Button>
          </div>
        )}
        {running && <SpeechJobCard job={running} />}
        <div className="flex items-center gap-1.5">
          <span className="text-[12px] text-muted">Pauses over</span>
          <NumberInput label="Shortest pause to show" value={pauseUs / US} min={0.1} max={5} step={0.1} format={(v) => v.toFixed(1)} onChange={(v) => useSpeech.setState({ pauseUs: Math.round(v * US) })} className="w-11" />
          <span className="text-[12px] text-muted">s</span>
          <span className="flex-1" />
          <Button
            className="h-7 px-2"
            disabled={pauses.length === 0 || !!cutBlocker}
            disabledReason={cutBlocker ?? `No pauses longer than ${formatDuration(pauseUs)}`}
            title={`Shorten every pause to ${formatDuration(pauseUs)}`}
            onClick={removeAll}
          >
            <Scissors size={14} /> {pauses.length > 0 ? `Remove ${pauses.length} pause${pauses.length === 1 ? "" : "s"} · ${formatDuration(total(pauses))}` : "Remove pauses"}
          </Button>
        </div>
        <KeepTracks project={project} />
        {!running && <JobError job={last} />}
        {errorNote}
      </div>
      <TranscriptText tokens={tokens} blocker={cutBlocker} onDelete={deleteRange} onCorrect={(i, text, shown) => (view ? correctWord(view.key, i, text, shown) : Promise.resolve(false))} />
    </div>
  );
}
