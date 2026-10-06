import { useEffect, useMemo } from "react";
import { AlertTriangle, RefreshCw, ScrollText, Scissors } from "lucide-react";
import { cutFromTranscript, formatSeconds, loadStoredTranscript, speechBlocker, startSpeech, tokenize, useSpeech, useTranscript, type Token } from "../../lib/speech";
import { keepsInPlace, mainClips, useEditor } from "../../lib/store";
import { US } from "../../lib/time";
import type { Clip, Project } from "../../lib/types";
import { Button, Checkbox, IconButton, NumberInput } from "../ui";
import { JobError, LANGUAGE_NAMES, SpeechFields, SpeechJobCard, useSpeechJobs } from "./SpeechControls";
import { TranscriptText } from "./TranscriptText";

/** Audio tracks with a checkbox: checked ones are not cut, so music keeps playing across cuts. */
function KeepTracks({ project }: { project: Project }) {
  const overrides = useEditor((s) => s.keepTracks);
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
          checked={keepsInPlace(project, t, overrides)}
          title="Checked: this track is not cut and keeps playing across cuts. Unchecked: it is cut together with the video."
          onChange={(v) => useEditor.setState({ keepTracks: { ...overrides, [t.id]: v } })}
        />
      ))}
    </div>
  );
}

/** "3 words (1.2 s)", or pauses when no word is included. */
const describe = (tokens: Token[], lengthUs: number) => {
  const words = tokens.filter((t) => t.kind === "word").length;
  const n = words || tokens.length;
  return `${n} ${words ? "word" : "pause"}${n === 1 ? "" : "s"} (${formatSeconds(lengthUs)})`;
};

const total = (tokens: Token[]) => tokens.reduce((s, t) => s + t.endUs - t.startUs, 0);

/** Text-based editing: the timeline's speech as text; deleting words cuts the video. */
export function TranscriptTab() {
  const project = useEditor((s) => s.snap!.project);
  const pauseUs = useSpeech((s) => s.pauseUs);
  const { words, language, stale } = useTranscript();
  const { running, last } = useSpeechJobs("transcript");
  const blocker = speechBlocker(project);
  const main = mainClips(project);
  const endUs = main.length > 0 ? main[main.length - 1].startUs + main[main.length - 1].durationUs : 0;
  const tokens = useMemo(() => (words ? tokenize(words, pauseUs, endUs) : []), [words, pauseUs, endUs]);
  const pauses = tokens.filter((t) => t.kind === "pause");
  const busy = !!running;

  useEffect(() => {
    if (!words) loadStoredTranscript();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const transcribe = () => startSpeech(null);
  const removePauses = () => cutFromTranscript(pauses, describe(pauses, total(pauses)));
  const deleteRange = async (lo: number, hi: number) => {
    const range = { startUs: tokens[lo].startUs, endUs: tokens[hi].endUs };
    const ok = await cutFromTranscript([range], describe(tokens.slice(lo, hi + 1), range.endUs - range.startUs));
    // The playhead lands on the cut, where what followed the deleted words now starts.
    if (ok) useEditor.getState().seek(range.startUs);
    return ok;
  };

  const start = running ? (
    <SpeechJobCard job={running} />
  ) : (
    <span className="flex" title={blocker ?? undefined}>
      <Button variant="primary" className="flex-1" disabled={!!blocker} onClick={transcribe}>
        {words ? <RefreshCw size={15} /> : <ScrollText size={15} />} {words ? "Refresh transcript" : "Transcribe timeline"}
      </Button>
    </span>
  );

  if (!words)
    return (
      <div className="flex flex-col gap-4 overflow-y-auto p-3">
        <p className="text-[12px] text-muted">Transcribe the timeline, then cut the video by deleting words.</p>
        <SpeechFields disabled={busy} />
        {start}
        {!running && <JobError job={last} />}
      </div>
    );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-col gap-3 border-b border-line p-3">
        {stale ? (
          <>
            <p className="flex gap-1.5 text-[12px] text-fg" role="status">
              <AlertTriangle size={14} className="mt-px shrink-0 text-warn" />
              The video changed after this transcript was made. Refresh it to edit by text again.
            </p>
            <SpeechFields disabled={busy} />
            {start}
          </>
        ) : (
          <>
            <div className="flex items-center gap-2">
              <span className="tabular flex-1 text-[12px] text-muted">
                {words.length} words{language ? ` · ${LANGUAGE_NAMES[language] ?? language}` : ""}
              </span>
              <IconButton label={blocker ?? (busy ? "Wait for speech recognition to finish" : "Transcribe again")} className="h-7 w-7" disabled={!!blocker || busy} onClick={transcribe}>
                <RefreshCw size={14} />
              </IconButton>
            </div>
            {running && <SpeechJobCard job={running} />}
            <div className="flex items-center gap-1.5">
              <span className="text-[12px] text-muted">Pauses over</span>
              <NumberInput label="Shortest pause to show" value={pauseUs / US} min={0.1} max={5} step={0.1} format={(v) => v.toFixed(1)} onChange={(v) => useSpeech.setState({ pauseUs: Math.round(v * US) })} className="w-11" />
              <span className="text-[12px] text-muted">s</span>
              <span className="flex-1" />
              <span title={pauses.length === 0 ? `No pauses longer than ${formatSeconds(pauseUs)}` : `Shorten every pause to ${formatSeconds(pauseUs)}`}>
                <Button className="h-7 px-2" disabled={pauses.length === 0} onClick={removePauses}>
                  <Scissors size={14} /> Remove {pauses.length > 0 ? `${pauses.length} · ${formatSeconds(total(pauses))}` : ""}
                </Button>
              </span>
            </div>
            <KeepTracks project={project} />
          </>
        )}
        {!running && <JobError job={last} />}
      </div>
      <TranscriptText tokens={tokens} disabled={stale} onDelete={deleteRange} />
    </div>
  );
}
