import { useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { ChevronLeft, ChevronRight, Diamond, Maximize, Minimize, RotateCcw } from "lucide-react";
import { clipOffset, keyframeIndexAt, keyframeTolerance, transformAt, upsertKeyframe } from "../../lib/keyframes";
import { LIMITS } from "../../lib/limits";
import { DEFAULT_TRANSFORM } from "../../lib/presets";
import { editClip, isCaptionTrack, setClipTransform, undoAction, useEditor } from "../../lib/store";
import type { Asset, Clip, EditCmd, Transform } from "../../lib/types";
import { Button, IconButton, NumberInput, Section, Slider } from "../ui";

function KeyframeControls({ clip, at }: { clip: Clip; at: AtPlayhead }) {
  const { seek } = useEditor.getState();
  const fps = useEditor((s) => s.snap!.project.canvas.fps);
  const tol = keyframeTolerance(fps);
  const { prevUs, nextUs, inside, atIndex } = at;
  const has = atIndex >= 0;

  // Built from the latest confirmed keyframes, so a keyframe added a moment ago is kept.
  const toggle = () =>
    editClip(clip.id, (c): EditCmd | EditCmd[] => {
      const offset = clipOffset(c, useEditor.getState().timeUs);
      const i = keyframeIndexAt(c, offset, tol);
      if (i < 0) return { type: "setKeyframes", clipId: c.id, keyframes: upsertKeyframe(c, offset, transformAt(c, c.content.transform, offset), tol) };
      const rest = c.keyframes.filter((_, j) => j !== i);
      const remove: EditCmd = { type: "setKeyframes", clipId: c.id, keyframes: rest };
      // Removing the last keyframe keeps its values as the clip's fixed transform.
      return rest.length > 0 ? remove : [{ type: "updateClip", clipId: c.id, transform: c.keyframes[i].transform }, remove];
    });

  return (
    <>
      <IconButton label="Previous keyframe" className="h-7 w-6" disabled={prevUs === null} onClick={() => prevUs !== null && seek(clip.startUs + prevUs)}>
        <ChevronLeft size={14} />
      </IconButton>
      <IconButton
        label={!inside ? "Move the playhead over this clip to add a keyframe" : has ? "Remove keyframe at playhead" : "Add keyframe at playhead"}
        className="h-7 w-7"
        active={has}
        disabled={!inside}
        onClick={toggle}
      >
        <Diamond size={14} fill={has ? "currentColor" : "none"} />
      </IconButton>
      <IconButton label="Next keyframe" className="h-7 w-6" disabled={nextUs === null} onClick={() => nextUs !== null && seek(clip.startUs + nextUs)}>
        <ChevronRight size={14} />
      </IconButton>
    </>
  );
}

/**
 * Punch-in or pull-out across the whole clip: a keyframe at the start and one at the end, with
 * the position and rotation at the playhead. Shows the first and last keyframe when there are
 * some, otherwise the current scale to 20 points more.
 */
function ZoomOverClip({ clip, scale }: { clip: Clip; scale: number }) {
  const [from, setFrom] = useState<number | null>(null);
  const [to, setTo] = useState<number | null>(null);
  const ks = clip.keyframes;
  const start = from ?? Math.round((ks.length > 0 ? ks[0].transform.scale : scale) * 100);
  const end = to ?? (ks.length > 1 ? Math.round(ks[ks.length - 1].transform.scale * 100) : start + 20);
  const apply = async () => {
    const replaced = clip.keyframes.length;
    const { timeUs, toast } = useEditor.getState();
    const done = await editClip(clip.id, (c) => {
      const base = transformAt(c, c.content.transform, clipOffset(c, timeUs));
      return {
        type: "setKeyframes",
        clipId: c.id,
        keyframes: [
          { tUs: 0, transform: { ...base, scale: start / 100 } },
          { tUs: c.durationUs, transform: { ...base, scale: end / 100 } },
        ],
      };
    });
    if (!done) return;
    // From now on the fields show the keyframes just written.
    setFrom(null);
    setTo(null);
    if (replaced > 0) toast({ kind: "info", text: `Replaced ${replaced} keyframe${replaced === 1 ? "" : "s"} with the zoom`, action: undoAction(done) });
  };
  const pct = (v: number) => String(Math.round(v));
  return (
    <div className="flex items-center gap-1.5 border-t border-line pt-3">
      <span className="shrink-0 text-[12px] text-muted">Zoom over clip</span>
      <span className="flex-1" />
      <NumberInput label="Zoom start scale" value={start} min={10} max={400} step={1} format={pct} onChange={setFrom} className="w-11" />
      <span className="text-[12px] text-muted" aria-hidden>
        to
      </span>
      <NumberInput label="Zoom end scale" value={end} min={10} max={400} step={1} format={pct} onChange={setTo} className="w-11" />
      <span className="text-[11px] text-muted">%</span>
      <Button className="h-7 px-2" title={`Keyframes at the clip's start (${start} %) and end (${end} %)`} onClick={apply}>
        Apply
      </Button>
    </div>
  );
}

/** What the playhead decides in the Transform section, as plain values. */
interface AtPlayhead extends Transform {
  inside: boolean;
  atIndex: number;
  prevUs: number | null;
  nextUs: number | null;
}

function atPlayhead(clip: Clip, timeUs: number, fps: number): AtPlayhead {
  const offset = clipOffset(clip, timeUs);
  const tol = keyframeTolerance(fps);
  const inside = timeUs >= clip.startUs && timeUs <= clip.startUs + clip.durationUs;
  return {
    ...transformAt(clip, clip.content.transform, offset),
    inside,
    atIndex: inside ? keyframeIndexAt(clip, offset, tol) : -1,
    prevUs: [...clip.keyframes].reverse().find((k) => k.tUs < offset - tol)?.tUs ?? null,
    nextUs: clip.keyframes.find((k) => k.tUs > offset + tol)?.tUs ?? null,
  };
}

export function TransformSection({ clip, asset }: { clip: Clip; asset?: Asset }) {
  const canvas = useEditor((s) => s.snap!.project.canvas);
  // Plain values, so playback re-renders the section only when a keyframed value moves or a keyframe is passed.
  const at = useEditor(useShallow((s) => atPlayhead(clip, s.timeUs, canvas.fps)));
  const { atIndex } = at;
  const transform = at;
  const set = (patch: Partial<Transform>, key: string) => setClipTransform(clip.id, patch, `${clip.id}:${key}`);
  // Scale that makes the media cover the whole canvas instead of fitting inside it.
  const fill =
    asset && asset.width > 0
      ? Math.max(canvas.width / asset.width, canvas.height / asset.height) / Math.min(canvas.width / asset.width, canvas.height / asset.height)
      : 1;
  const keyed = clip.keyframes.length > 0;

  // New text is centred; captions sit low in the frame, where the engine puts them.
  const reset = () =>
    editClip(clip.id, (c, track) => [
      { type: "updateClip", clipId: c.id, transform: { ...DEFAULT_TRANSFORM, y: isCaptionTrack(track) ? LIMITS.captionY : 0 } },
      { type: "setKeyframes", clipId: c.id, keyframes: [] },
    ]);

  return (
    <Section
      title="Transform"
      actions={
        <>
          <KeyframeControls clip={clip} at={at} />
          <span className="mx-0.5 h-4 w-px bg-line" />
          <IconButton label={keyed ? "Reset transform and remove keyframes" : "Reset transform"} className="h-7 w-7" onClick={reset}>
            <RotateCcw size={14} />
          </IconButton>
        </>
      }
    >
      {keyed && (
        <p className="tabular -mt-1 text-[12px] text-muted">
          {clip.keyframes.length} keyframe{clip.keyframes.length === 1 ? "" : "s"} · edits {atIndex >= 0 ? "update this keyframe" : "add a keyframe here"}
        </p>
      )}
      {asset && asset.kind !== "audio" && (
        <div className="grid grid-cols-2 gap-2">
          <Button onClick={() => set({ scale: 1, x: 0, y: 0 }, "fit")}>
            <Minimize size={14} /> Fit
          </Button>
          <Button onClick={() => set({ scale: Math.round(fill * 1000) / 1000, x: 0, y: 0 }, "fill")}>
            <Maximize size={14} /> Fill
          </Button>
        </div>
      )}
      <Slider label="Scale" value={transform.scale * 100} min={10} max={400} step={1} unit="%" format={(v) => String(Math.round(v))} onChange={(v) => set({ scale: v / 100 }, "scale")} />
      <Slider label="Position X" value={transform.x * 100} min={-100} max={100} step={0.5} unit="%" format={(v) => v.toFixed(1)} onChange={(v) => set({ x: v / 100 }, "x")} />
      <Slider label="Position Y" value={transform.y * 100} min={-100} max={100} step={0.5} unit="%" format={(v) => v.toFixed(1)} onChange={(v) => set({ y: v / 100 }, "y")} />
      <Slider label="Rotation" value={transform.rotation} min={-180} max={180} step={1} unit="°" format={(v) => String(Math.round(v))} onChange={(v) => set({ rotation: v }, "rot")} />
      <Slider label="Opacity" value={transform.opacity * 100} min={0} max={100} step={1} unit="%" format={(v) => String(Math.round(v))} onChange={(v) => set({ opacity: v / 100 }, "opacity")} />
      <ZoomOverClip clip={clip} scale={transform.scale} />
    </Section>
  );
}
