import { ChevronLeft, ChevronRight, Diamond, Maximize, Minimize, RotateCcw } from "lucide-react";
import { clipOffset, keyframeIndexAt, keyframeTolerance, transformAt, upsertKeyframe } from "../../lib/keyframes";
import { CAPTION_Y, DEFAULT_TRANSFORM } from "../../lib/presets";
import { editClip, isCaptionTrack, setClipTransform, useEditor } from "../../lib/store";
import type { Asset, Clip, EditCmd, Transform } from "../../lib/types";
import { Button, IconButton, Section, Slider } from "../ui";

function KeyframeControls({ clip, offset, inside, atIndex }: { clip: Clip; offset: number; inside: boolean; atIndex: number }) {
  const { seek } = useEditor.getState();
  const fps = useEditor((s) => s.snap!.project.canvas.fps);
  const tol = keyframeTolerance(fps);
  const prev = [...clip.keyframes].reverse().find((k) => k.tUs < offset - tol);
  const next = clip.keyframes.find((k) => k.tUs > offset + tol);
  const has = atIndex >= 0;

  // Built from the latest confirmed keyframes, so a keyframe added a moment ago is kept.
  const toggle = () =>
    editClip(clip.id, (c): EditCmd | EditCmd[] => {
      const i = keyframeIndexAt(c, offset, tol);
      if (i < 0) return { type: "setKeyframes", clipId: c.id, keyframes: upsertKeyframe(c, offset, transformAt(c, c.content.transform, offset), tol) };
      const rest = c.keyframes.filter((_, j) => j !== i);
      const remove: EditCmd = { type: "setKeyframes", clipId: c.id, keyframes: rest };
      // Removing the last keyframe keeps its values as the clip's fixed transform.
      return rest.length > 0 ? remove : [{ type: "updateClip", clipId: c.id, transform: c.keyframes[i].transform }, remove];
    });

  return (
    <>
      <IconButton label="Previous keyframe" className="h-7 w-6" disabled={!prev} onClick={() => prev && seek(clip.startUs + prev.tUs)}>
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
      <IconButton label="Next keyframe" className="h-7 w-6" disabled={!next} onClick={() => next && seek(clip.startUs + next.tUs)}>
        <ChevronRight size={14} />
      </IconButton>
    </>
  );
}

export function TransformSection({ clip, asset }: { clip: Clip; asset?: Asset }) {
  const canvas = useEditor((s) => s.snap!.project.canvas);
  const timeUs = useEditor((s) => s.timeUs);
  const offset = clipOffset(clip, timeUs);
  const inside = timeUs >= clip.startUs && timeUs <= clip.startUs + clip.durationUs;
  const atIndex = inside ? keyframeIndexAt(clip, offset, keyframeTolerance(canvas.fps)) : -1;
  const transform = transformAt(clip, clip.content.transform, offset);
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
      { type: "updateClip", clipId: c.id, transform: { ...DEFAULT_TRANSFORM, y: isCaptionTrack(track) ? CAPTION_Y : 0 } },
      { type: "setKeyframes", clipId: c.id, keyframes: [] },
    ]);

  return (
    <Section
      title="Transform"
      actions={
        <>
          <KeyframeControls clip={clip} offset={offset} inside={inside} atIndex={atIndex} />
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
    </Section>
  );
}
