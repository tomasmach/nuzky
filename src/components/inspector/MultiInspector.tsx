import { useShallow } from "zustand/react/shallow";
import { Copy, Layers, RotateCcw, Trash2 } from "lucide-react";
import { LIMITS } from "../../lib/limits";
import { ADJUST_ROWS, DEFAULT_TRANSFORM, NO_ADJUST, sameAdjust } from "../../lib/presets";
import { deleteSelection, duplicateSelection, editClips, findClip, isCaptionTrack, readoutTime, transformAtPlayhead, useEditor } from "../../lib/store";
import type { Adjust, Clip, EditCmd, Project, Track, Transform } from "../../lib/types";
import { FontPicker } from "../FontPicker";
import { Button, IconButton, Section, Slider } from "../ui";
import { InspectorHeader, QUIET } from "./Header";
import { CleanVoiceRow } from "./MediaSections";
import { FieldRow } from "./TextSection";

type Found = { clip: Clip; track: Track };

/** What a clip offers to a shared edit. */
function facts(project: Project, { clip, track }: Found) {
  const c = clip.content;
  const asset = c.type === "media" ? project.assets.find((a) => a.id === c.assetId) : undefined;
  const onAudioTrack = track.kind === "audio";
  return {
    transform: !onAudioTrack,
    adjust: c.type === "media" && !onAudioTrack && asset?.kind !== "audio",
    sound: c.type === "media" && (onAudioTrack || !!asset?.hasAudio),
    text: c.type === "text",
    kind: c.type === "text" ? (isCaptionTrack(track) ? "caption" : "text") : onAudioTrack ? "audio" : (asset?.kind ?? "video"),
  };
}

/** The shared value, or mixed when the clips differ. */
function shared(values: number[]) {
  return { value: values[0] ?? 0, mixed: values.some((v) => Math.abs(v - values[0]) > 1e-4) };
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Nouns for the selection summary: "2 videos · 3 captions · 1 audio clip". */
const KIND_NOUNS: Record<string, string> = { video: "video", image: "image", caption: "caption", text: "text clip", audio: "audio clip" };

const KEYS = ["x", "y", "scale", "rotation", "opacity"] as const;

function TransformRows({ found, coalesce }: { found: Found[]; coalesce: string }) {
  const ids = found.map((f) => f.clip.id);
  // Plain numbers, so playback re-renders the rows only when a keyframed value moves.
  const flat = useEditor(
    useShallow((s) =>
      found.flatMap((f) => {
        const t = transformAtPlayhead(f.clip, readoutTime(s));
        return KEYS.map((k) => t[k]);
      }),
    ),
  );
  const field = (k: keyof Transform) => shared(found.map((_, i) => flat[i * KEYS.length + KEYS.indexOf(k)]));
  const keyed = found.filter((f) => f.clip.keyframes.length > 0).length;
  // One absolute value for every clip, keyframes included, so "110 % everywhere" really is everywhere.
  const set = (patch: Partial<Transform>, key: string) =>
    editClips(
      ids,
      (clip): EditCmd[] => {
        const cmds: EditCmd[] = [{ type: "updateClip", clipId: clip.id, transform: { ...clip.content.transform, ...patch } }];
        if (clip.keyframes.length > 0) cmds.push({ type: "setKeyframes", clipId: clip.id, keyframes: clip.keyframes.map((k) => ({ ...k, transform: { ...k.transform, ...patch } })) });
        return cmds;
      },
      `${coalesce}:${key}`,
    );
  const reset = () =>
    editClips(ids, (clip, track) => [
      { type: "updateClip", clipId: clip.id, transform: { ...DEFAULT_TRANSFORM, y: isCaptionTrack(track) ? LIMITS.captionY : 0 } },
      { type: "setKeyframes", clipId: clip.id, keyframes: [] },
    ]);
  const scale = field("scale");
  const x = field("x");
  const y = field("y");
  const rotation = field("rotation");
  const opacity = field("opacity");
  return (
    <Section
      title="Transform"
      actions={
        <IconButton label="Reset transform of all selected clips" className={QUIET} onClick={reset}>
          <RotateCcw size={14} />
        </IconButton>
      }
    >
      {keyed > 0 && <p className="-mt-1 text-[12px] text-muted">{plural(keyed, "clip")} with keyframes: changes apply to every keyframe.</p>}
      <Slider label="Scale" mixed={scale.mixed} value={scale.value * 100} min={10} max={400} step={1} unit="%" format={(v) => String(Math.round(v))} onChange={(v) => set({ scale: v / 100 }, "scale")} />
      <Slider label="Position X" mixed={x.mixed} value={x.value * 100} min={-100} max={100} step={0.5} unit="%" format={(v) => v.toFixed(1)} onChange={(v) => set({ x: v / 100 }, "x")} />
      <Slider label="Position Y" mixed={y.mixed} value={y.value * 100} min={-100} max={100} step={0.5} unit="%" format={(v) => v.toFixed(1)} onChange={(v) => set({ y: v / 100 }, "y")} />
      <Slider label="Rotation" mixed={rotation.mixed} value={rotation.value} min={-180} max={180} step={1} unit="°" format={(v) => String(Math.round(v))} onChange={(v) => set({ rotation: v }, "rot")} />
      <Slider label="Opacity" mixed={opacity.mixed} value={opacity.value * 100} min={0} max={100} step={1} unit="%" format={(v) => String(Math.round(v))} onChange={(v) => set({ opacity: v / 100 }, "opacity")} />
    </Section>
  );
}

const adjustOf = (clip: Clip) => (clip.content.type === "media" ? clip.content.adjust : NO_ADJUST);

function AdjustRows({ found, coalesce }: { found: Found[]; coalesce: string }) {
  const ids = found.map((f) => f.clip.id);
  const set = (key: keyof Adjust, v: number) =>
    editClips(ids, (c) => (c.content.type === "media" ? { type: "updateClip", clipId: c.id, adjust: { ...c.content.adjust, [key]: v / 100 } } : null), `${coalesce}:adjust:${key}`);
  const untouched = found.every((f) => sameAdjust(adjustOf(f.clip), NO_ADJUST));
  return (
    <Section
      title="Adjust"
      actions={
        <IconButton label="Reset adjustments of all selected clips" className={QUIET} disabled={untouched} onClick={() => editClips(ids, (c) => ({ type: "updateClip", clipId: c.id, adjust: NO_ADJUST }))}>
          <RotateCcw size={14} />
        </IconButton>
      }
    >
      {ADJUST_ROWS.map((r) => {
        const s = shared(found.map((f) => adjustOf(f.clip)[r.key]));
        return <Slider key={r.key} label={r.label} mixed={s.mixed} value={s.value * 100} min={r.min} max={100} step={1} format={(v) => String(Math.round(v))} onChange={(v) => set(r.key, v)} />;
      })}
    </Section>
  );
}

function VolumeRow({ found, coalesce }: { found: Found[]; coalesce: string }) {
  const ids = found.map((f) => f.clip.id);
  const s = shared(found.map((f) => (f.clip.content.type === "media" ? f.clip.content.volume : 0)));
  return (
    <Section title="Audio">
      <Slider
        label="Volume"
        mixed={s.mixed}
        value={s.value * 100}
        min={0}
        max={200}
        step={1}
        unit="%"
        format={(v) => String(Math.round(v))}
        onChange={(v) => editClips(ids, (c) => ({ type: "updateClip", clipId: c.id, volume: v / 100 }), `${coalesce}:volume`)}
      />
      <CleanVoiceRow clips={found.map((f) => f.clip)} />
    </Section>
  );
}

function FontRow({ found }: { found: Found[] }) {
  const fonts = found.map((f) => (f.clip.content.type === "text" ? (f.clip.content.style.fontFamily ?? null) : null));
  const mixed = fonts.some((f) => f !== fonts[0]);
  const ids = found.map((f) => f.clip.id);
  return (
    <Section title="Text">
      <FieldRow label="Font">
        <FontPicker
          value={fonts[0]}
          mixed={mixed}
          onChange={(fontFamily) => editClips(ids, (c) => (c.content.type === "text" ? { type: "updateClip", clipId: c.id, style: { ...c.content.style, fontFamily } } : null))}
        />
      </FieldRow>
    </Section>
  );
}

/** Several clips: the controls they all share, each change applied to all of them as one undo step. */
export function MultiInspector({ ids }: { ids: string[] }) {
  const project = useEditor((s) => s.snap!.project);
  const found = ids.map((id) => findClip(project, id)).filter((f): f is Found => !!f);
  const info = found.map((f) => facts(project, f));
  const all = (k: "transform" | "adjust" | "sound" | "text") => found.length > 0 && info.every((i) => i[k]);
  const counts = new Map<string, number>();
  for (const i of info) counts.set(i.kind, (counts.get(i.kind) ?? 0) + 1);
  const detail = [...counts].map(([kind, n]) => plural(n, KIND_NOUNS[kind] ?? kind)).join(" · ");
  // Scoped to this selection, so a drag is one undo step and never merges with another selection's.
  const coalesce = `multi:${ids.join(",")}`;
  return (
    <>
      <InspectorHeader icon={Layers} title={`${found.length} clips selected`} detail={detail} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="grid grid-cols-2 gap-2 border-b border-white/[.07] px-4 pb-4 pt-1">
          <Button onClick={duplicateSelection} title="Duplicate (Ctrl+D)">
            <Copy size={14} /> Duplicate
          </Button>
          <Button variant="danger" onClick={deleteSelection} title="Delete (Delete)">
            <Trash2 size={14} /> Delete
          </Button>
        </div>
        {all("transform") && <TransformRows found={found} coalesce={coalesce} />}
        {all("text") && <FontRow found={found} />}
        {all("adjust") && <AdjustRows found={found} coalesce={coalesce} />}
        {all("sound") && <VolumeRow found={found} coalesce={coalesce} />}
      </div>
    </>
  );
}
