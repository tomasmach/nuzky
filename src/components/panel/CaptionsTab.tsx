import { Captions } from "lucide-react";
import { CAPTION_STYLES, sameStyle } from "../../lib/presets";
import { speechBlocker, startSpeech, useSpeech } from "../../lib/speech";
import { applyCaptionFont, applyCaptionStyle, isCaptionTrack, useEditor } from "../../lib/store";
import { FontPicker } from "../FontPicker";
import { Button, Segmented, TextSwatch } from "../ui";
import { JobError, SpeechFields, SpeechJobCard, useSpeechJobs } from "./SpeechControls";

const WORDS = [
  { id: 1, label: "1", title: "One word at a time" },
  { id: 2, label: "2", title: "Up to two words, at most 15 characters, like reels" },
  { id: 3, label: "3", title: "Up to three words, at most 15 characters" },
  { id: 0, label: "Phrases", title: "Whole phrases" },
];

export function CaptionsTab() {
  const captionWords = useSpeech((s) => s.captionWords);
  const styleIdx = useSpeech((s) => s.captionStyle);
  const newFont = useSpeech((s) => s.captionFont);
  const captionStyle = useEditor((s) => {
    const first = s.snap?.project.tracks.find(isCaptionTrack)?.clips[0]?.content;
    return first?.type === "text" ? first.style : null;
  });
  const { running, last } = useSpeechJobs("captions");
  const blocker = useEditor((s) => (s.snap ? speechBlocker(s.snap.project) : null));
  const hasCaptions = captionStyle !== null;
  // With captions on the timeline, the highlighted style and the font are the ones they use.
  const current = hasCaptions ? CAPTION_STYLES.findIndex((s) => sameStyle(s.style, captionStyle)) : styleIdx;
  const font = hasCaptions ? (captionStyle.fontFamily ?? null) : newFont;

  // Regenerating keeps the look of the captions already there, edits included.
  const start = () => startSpeech({ style: captionStyle ?? { ...CAPTION_STYLES[styleIdx].style, fontFamily: newFont } });
  const pickStyle = (i: number) => {
    useSpeech.setState({ captionStyle: i });
    if (hasCaptions) applyCaptionStyle({ ...CAPTION_STYLES[i].style, fontFamily: font });
  };
  const pickFont = (f: string) => (hasCaptions ? applyCaptionFont(f) : useSpeech.setState({ captionFont: f }));
  const busy = !!running;

  return (
    <div className="flex flex-col gap-4 overflow-y-auto p-3">
      <SpeechFields disabled={busy} />
      <div className="flex flex-col gap-1.5">
        <span className="text-[12px] text-muted">{hasCaptions ? "Words per caption · applies when you regenerate" : "Words per caption"}</span>
        <Segmented
          label="Words per caption"
          value={captionWords}
          onChange={(n) => useSpeech.setState({ captionWords: n })}
          options={WORDS}
          disabled={busy}
          disabledReason="Wait for speech recognition to finish"
        />
      </div>
      <div className="flex flex-col gap-1.5">
        <span className="text-[12px] text-muted">Style</span>
        <div className="grid grid-cols-3 gap-2">
          {CAPTION_STYLES.map((s, i) => (
            <button
              key={s.name}
              type="button"
              aria-pressed={i === current}
              disabled={busy}
              title={busy ? "Wait for speech recognition to finish" : hasCaptions ? `Apply ${s.name} to all captions` : `Use ${s.name} for new captions`}
              onClick={() => pickStyle(i)}
              className={`flex h-12 min-w-0 items-center justify-center rounded-md border bg-line disabled:cursor-not-allowed disabled:opacity-40 ${
                i === current ? "border-accent shadow-[0_0_0_1px_var(--color-accent)]" : "border-line enabled:hover:border-muted"
              }`}
            >
              <TextSwatch style={{ ...s.style, fontFamily: font }} label={s.name} />
            </button>
          ))}
        </div>
      </div>
      <div className="flex flex-col gap-1.5">
        <span className="text-[12px] text-muted">Font</span>
        {/* A row, so the picker's flex-1 fills the width instead of collapsing the column. */}
        <div className="flex">
          <FontPicker value={font} onChange={pickFont} disabled={busy} disabledReason="Wait for speech recognition to finish" />
        </div>
      </div>
      {running ? (
        <SpeechJobCard job={running} />
      ) : (
        <span className="flex" title={blocker ?? undefined}>
          <Button variant="primary" className="flex-1" disabled={!!blocker} onClick={start}>
            <Captions size={15} /> {hasCaptions ? "Regenerate captions" : "Generate captions"}
          </Button>
        </span>
      )}
      {!running && <JobError job={last} />}
    </div>
  );
}
