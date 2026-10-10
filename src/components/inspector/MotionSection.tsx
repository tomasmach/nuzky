import { useEffect } from "react";
import { Ban, MoveRight, ZoomIn, ZoomOut, type LucideIcon } from "lucide-react";
import { api, errorText } from "../../lib/api";
import { clipOffset, motionOf, transformAt } from "../../lib/keyframes";
import { useSpeech } from "../../lib/speech";
import { editClip, undoAction, useEditor } from "../../lib/store";
import { US } from "../../lib/time";
import type { Clip, EditCmd, MotionKind, TimeRange } from "../../lib/types";
import { PresetTile, Section, Segmented } from "../ui";

const MOTIONS: { kind: MotionKind; label: string; title: string; icon: LucideIcon }[] = [
  { kind: "pushIn", label: "Push in", title: "Slowly zoom in", icon: ZoomIn },
  { kind: "pullOut", label: "Pull out", title: "Start zoomed in and slowly zoom out to the clip's framing", icon: ZoomOut },
  { kind: "kenBurns", label: "Ken Burns", title: "Slowly zoom in while drifting right", icon: MoveRight },
];
const STRENGTHS = [0.06, 0.1, 0.15];
/** A pause this long ends a sentence, as in the engine's emphasis analysis. */
const SENTENCE_GAP_US = 600_000;

type Over = "clip" | "sentence";
/** The strength of the motion shown last, for the next preset, also on another clip. */
let lastStrength = 0.1;

/** From the playhead to the end of the sentence said there, inside the clip; or why not. */
async function sentenceRange(clip: Clip): Promise<TimeRange | string> {
  const { timeUs, snap } = useEditor.getState();
  const end = clip.startUs + clip.durationUs;
  const frame = US / snap!.project.canvas.fps;
  if (timeUs < clip.startUs || timeUs > end - frame) return "Move the playhead over this clip first.";
  let view;
  try {
    view = await api.transcriptView(useSpeech.getState().pauseUs);
  } catch (e) {
    return errorText(e);
  }
  const words = view.words.filter((w) => w.endUs > timeUs);
  if (words.length === 0 || words[0].startUs >= end)
    return view.untranscribed.length > 0 ? "Transcribe the timeline first, so the sentence end is known." : "No speech after the playhead in this clip.";
  const last = words.find((w, i) => /[.!?…]\s*$/.test(w.text) || (i + 1 < words.length && words[i + 1].startUs - w.endUs >= SENTENCE_GAP_US));
  return { startUs: Math.round(timeUs), endUs: Math.min(end, (last ?? words[words.length - 1]).endUs) };
}

/**
 * Writes the motion as one undo step, or removes it with `kind` null. Keyframes of the user's own
 * are replaced, starting from the picture at the playhead, with a toast that offers Undo.
 */
async function applyMotion(clip: Clip, kind: MotionKind | null, strength: number, over: Over | TimeRange) {
  const { timeUs, toast } = useEditor.getState();
  let range: TimeRange | null = typeof over === "object" ? over : null;
  if (kind && over === "sentence") {
    const found = await sentenceRange(clip);
    if (typeof found === "string") {
      toast({ kind: "error", text: found });
      return;
    }
    range = found;
  }
  let own = 0;
  const done = await editClip(clip.id, (c, _track, project) => {
    own = c.keyframes.length > 0 && !motionOf(c, project.canvas) ? c.keyframes.length : 0;
    const cmds: EditCmd[] = [];
    if (own > 0) cmds.push({ type: "updateClip", clipId: c.id, transform: transformAt(c, c.content.transform, clipOffset(c, timeUs)) });
    if (c.keyframes.length > 0) cmds.push({ type: "setKeyframes", clipId: c.id, keyframes: [] });
    if (kind) cmds.push({ type: "applyMotion", clipId: c.id, range, kind, strength });
    return cmds.length > 0 ? cmds : null;
  });
  if (!done || own === 0) return;
  const n = `${own} keyframe${own === 1 ? "" : "s"}`;
  const label = MOTIONS.find((m) => m.kind === kind)?.label;
  toast({ kind: "info", text: label ? `Replaced ${n} with ${label}` : `Removed ${n}`, action: undoAction(done) });
}

/** The clip's thumbnail moving as the preset moves the picture, and its direction in a corner chip, which tells the tiles apart while still. */
function MotionPreview({ kind, icon: Icon, thumb }: { kind: MotionKind; icon: LucideIcon; thumb: string | null }) {
  return (
    <>
      <span
        className={`pv m-${kind} absolute inset-0 bg-clip-video bg-cover bg-center`}
        style={{ animationName: kind === "pullOut" ? "m-pullOut" : "m-pushIn", ...(thumb ? { backgroundImage: `url(${thumb})` } : {}) }}
      />
      <span className="absolute bottom-1 right-1 flex h-4 w-4 items-center justify-center rounded-[4px] bg-black/60 text-fg">
        <Icon size={11} />
      </span>
    </>
  );
}

export function MotionSection({ clip }: { clip: Clip }) {
  const canvas = useEditor((s) => s.snap!.project.canvas);
  const thumb = useEditor((s) => (clip.content.type === "media" ? (s.thumbs[clip.content.assetId] ?? null) : null));
  const motion = motionOf(clip, canvas);
  const ks = clip.keyframes;
  // The section shows the clip's motion; changing the strength or the range rewrites it. A new
  // preset runs over the whole clip at the strength shown last.
  const strength = motion?.strength ?? lastStrength;
  const over: Over = motion && (ks[0].tUs > 0 || ks[1].tUs < clip.durationUs) ? "sentence" : "clip";
  const kept: TimeRange | null = motion && { startUs: clip.startUs + ks[0].tUs, endUs: clip.startUs + ks[1].tUs };
  // Another preset keeps the range of the motion it replaces.
  const range = kept ?? "clip";
  const shown = motion?.strength;
  useEffect(() => {
    if (shown !== undefined) lastStrength = shown;
  }, [shown]);

  return (
    <Section title="Motion">
      <div className="grid grid-cols-4 gap-2">
        <PresetTile label="None" selected={ks.length === 0} title="No motion" onClick={() => applyMotion(clip, null, strength, range)}>
          <span className="absolute inset-0 flex items-center justify-center text-muted">
            <Ban size={18} />
          </span>
        </PresetTile>
        {MOTIONS.map((m) => (
          <PresetTile key={m.kind} label={m.label} title={m.title} selected={motion?.kind === m.kind} onClick={() => applyMotion(clip, m.kind, strength, range)}>
            <MotionPreview kind={m.kind} icon={m.icon} thumb={thumb} />
          </PresetTile>
        ))}
      </div>
      <div className="flex flex-col gap-0.5">
        <span className="text-[12px] leading-4 text-muted">Strength</span>
        <Segmented
          label="Motion strength"
          disabled={!motion}
          disabledReason="Pick a motion first"
          value={STRENGTHS.find((s) => Math.abs(s - strength) < 1e-3) ?? null}
          onChange={(s) => motion && kept && s !== motion.strength && applyMotion(clip, motion.kind, s, kept)}
          options={STRENGTHS.map((s) => ({ id: s, label: `${Math.round(s * 100)}%`, title: `Zoom ${Math.round(s * 100)}%` }))}
        />
      </div>
      <div className="flex flex-col gap-0.5">
        <span className="text-[12px] leading-4 text-muted">Range</span>
        <Segmented
          label="Motion range"
          disabled={!motion}
          disabledReason="Pick a motion first"
          value={over}
          onChange={(o) => motion && (o !== over || o === "sentence") && applyMotion(clip, motion.kind, motion.strength, o)}
          options={[
            { id: "clip", label: "Whole clip", title: "Over the whole clip" },
            { id: "sentence", label: "Rest of sentence", title: "From the playhead to the end of the sentence said there" },
          ]}
        />
      </div>
    </Section>
  );
}
