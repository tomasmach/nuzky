import { useShallow } from "zustand/react/shallow";
import { RotateCcw } from "lucide-react";
import { NO_CROP, NO_SHAPE } from "../../lib/presets";
import { editClip, readoutTime, setClipTransform, transformAtPlayhead, transformEdit, useEditor } from "../../lib/store";
import type { Asset, Clip, Crop, EditCmd, Shape } from "../../lib/types";
import { ColorInput, IconButton, Section, Segmented, Slider } from "../ui";
import { QUIET } from "./Header";

type Media = Extract<Clip["content"], { type: "media" }>;
type Kind = "rectangle" | "rounded" | "circle";

/** Every edge keeps at least this much of the picture between it and the opposite one. */
const MIN_VISIBLE = 0.05;
const ROUNDED = 0.2;
const pct = (v: number) => String(Math.round(v));

const kindOf = (radius: number): Kind => (radius <= 0 ? "rectangle" : radius >= 1 ? "circle" : "rounded");

/** The crop cut further on its longer side, around its centre, so what is left is square. */
export function squareCrop(crop: Crop, asset: Asset): Crop {
  const w = asset.width * (1 - crop.left - crop.right);
  const h = asset.height * (1 - crop.top - crop.bottom);
  if (w > h) {
    const cut = (w - h) / 2 / asset.width;
    return { ...crop, left: crop.left + cut, right: crop.right + cut };
  }
  const cut = (h - w) / 2 / asset.height;
  return { ...crop, top: crop.top + cut, bottom: crop.bottom + cut };
}

/** Sets one edge, keeping a strip of the picture between it and the opposite edge. */
export function withEdge(crop: Crop, edge: keyof Crop, value: number): Crop {
  const opposite = { left: "right", right: "left", top: "bottom", bottom: "top" } as const;
  return { ...crop, [edge]: Math.max(0, Math.min(1 - MIN_VISIBLE - crop[opposite[edge]], value)) };
}

const EDGES: { key: keyof Crop; label: string }[] = [
  { key: "left", label: "Crop left" },
  { key: "right", label: "Crop right" },
  { key: "top", label: "Crop top" },
  { key: "bottom", label: "Crop bottom" },
];

export function ShapeSection({ clip, content, asset }: { clip: Clip; content: Media; asset?: Asset }) {
  // Plain values, so playback re-renders the section only when a keyframed crop moves.
  const crop = useEditor(useShallow((s) => transformAtPlayhead(clip, readoutTime(s)).crop ?? NO_CROP));
  const shape = content.shape ?? NO_SHAPE;
  const keyed = clip.keyframes.length > 0;
  const plain = !content.shape && Object.values(crop).every((v) => v === 0) && !clip.keyframes.some((k) => k.transform.crop);

  const setEdge = (edge: keyof Crop, v: number) =>
    setClipTransform(clip.id, (t) => ({ crop: withEdge(t.crop ?? NO_CROP, edge, v / 100) }), `${clip.id}:crop:${edge}`);
  // Built from the latest confirmed shape, so a second slider never overwrites the first.
  const setShape = (patch: Partial<Shape>, key: string) =>
    editClip(clip.id, (c) => (c.content.type === "media" ? { type: "updateClip", clipId: c.id, shape: { ...NO_SHAPE, ...c.content.shape, ...patch } } : null), `${clip.id}:shape:${key}`);
  // A circle needs a square: picking it crops the longer side too, in the same undo step.
  const pickKind = (kind: Kind) =>
    editClip(clip.id, (c, _track, project): EditCmd[] | null => {
      if (c.content.type !== "media") return null;
      const radius = kind === "circle" ? 1 : kind === "rounded" ? ROUNDED : 0;
      const cmds: EditCmd[] = [{ type: "updateClip", clipId: c.id, shape: { ...NO_SHAPE, ...c.content.shape, radius } }];
      if (kind === "circle" && asset) cmds.push(transformEdit(c, project, useEditor.getState().timeUs, (t) => ({ crop: squareCrop(t.crop ?? NO_CROP, asset) })));
      return cmds;
    });
  // Shows the whole picture with plain edges again, in every keyframe too.
  const reset = () =>
    editClip(clip.id, (c): EditCmd[] => {
      const whole = <T extends { crop?: Crop | null }>(t: T) => ({ ...t, crop: null });
      const cmds: EditCmd[] = [{ type: "updateClip", clipId: c.id, transform: whole(c.content.transform), shape: NO_SHAPE }];
      if (c.keyframes.length > 0) cmds.push({ type: "setKeyframes", clipId: c.id, keyframes: c.keyframes.map((k) => ({ ...k, transform: whole(k.transform) })) });
      return cmds;
    });
  const kind = kindOf(shape.radius);

  return (
    <Section
      title="Crop and shape"
      actions={
        <IconButton label="Show the whole picture with square corners" className={QUIET} disabled={plain} onClick={reset}>
          <RotateCcw size={14} />
        </IconButton>
      }
    >
      {keyed && <p className="-mt-1 text-[12px] text-muted">The crop follows the transform keyframes</p>}
      {EDGES.map((e) => (
        <Slider key={e.key} label={e.label} value={crop[e.key] * 100} min={0} max={95} step={0.5} unit="%" format={pct} onChange={(v) => setEdge(e.key, v)} />
      ))}
      <Segmented
        label="Shape"
        value={kind}
        onChange={pickKind}
        options={[
          { id: "rectangle", label: "Rectangle", title: "Square corners" },
          { id: "rounded", label: "Rounded", title: "Rounded corners" },
          { id: "circle", label: "Circle", title: "Round, with the crop made square" },
        ]}
      />
      {kind === "rounded" && (
        <Slider label="Corners" value={shape.radius * 100} min={1} max={99} step={1} unit="%" format={pct} onChange={(v) => setShape({ radius: v / 100 }, "radius")} />
      )}
      <Slider label="Border" value={shape.borderWidth} min={0} max={40} step={1} unit="px" format={pct} onChange={(v) => setShape({ borderWidth: v }, "border")} />
      {shape.borderWidth > 0 && <ColorInput label="Border colour" value={shape.borderColor} onChange={(v) => setShape({ borderColor: v }, "borderColor")} />}
      <Slider label="Shadow" value={shape.shadow * 100} min={0} max={100} step={1} unit="%" format={pct} onChange={(v) => setShape({ shadow: v / 100 }, "shadow")} />
    </Section>
  );
}
