import { Maximize, Minimize, RotateCcw, SlidersHorizontal } from "lucide-react";
import { findClip, useEditor } from "../lib/store";
import { formatDuration } from "../lib/time";
import type { Asset, Clip, TextStyle, Transform } from "../lib/types";
import { CAPTION_STYLES, PresetSwatch, TEXT_PRESETS } from "./MediaPanel";
import { FORMATS } from "./TopBar";
import { Button, ColorInput, IconButton, Section, Slider } from "./ui";

const DEFAULT_TRANSFORM: Transform = { x: 0, y: 0, scale: 1, rotation: 0, opacity: 1 };

function TransformSection({ clip, transform, asset }: { clip: Clip; transform: Transform; asset?: Asset }) {
  const { edit } = useEditor.getState();
  const canvas = useEditor((s) => s.snap!.project.canvas);
  const set = (patch: Partial<Transform>, key: string) =>
    edit({ type: "updateClip", clipId: clip.id, transform: { ...transform, ...patch } }, `${clip.id}:${key}`);
  // Scale that makes the media cover the whole canvas instead of fitting inside it.
  const fill = asset && asset.width > 0 ? Math.max(canvas.width / asset.width, canvas.height / asset.height) / Math.min(canvas.width / asset.width, canvas.height / asset.height) : 1;

  return (
    <Section
      title="Transform"
      actions={
        <IconButton label="Reset transform" onClick={() => edit({ type: "updateClip", clipId: clip.id, transform: { ...DEFAULT_TRANSFORM, y: clip.content.type === "text" ? transform.y : 0 } })}>
          <RotateCcw size={14} />
        </IconButton>
      }
    >
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

function TextSection({ clip, text, style }: { clip: Clip; text: string; style: TextStyle }) {
  const { edit } = useEditor.getState();
  const setStyle = (patch: Partial<TextStyle>, key: string) => edit({ type: "updateClip", clipId: clip.id, style: { ...style, ...patch } }, `${clip.id}:style:${key}`);
  return (
    <>
      <Section title="Text">
        <textarea
          aria-label="Text"
          value={text}
          rows={3}
          onChange={(e) => edit({ type: "updateClip", clipId: clip.id, text: e.target.value }, `${clip.id}:text`)}
          className="resize-y rounded-md border border-line bg-raised p-2 text-[13px] text-fg outline-none focus:border-accent"
        />
        <div className="grid grid-cols-3 gap-1.5">
          {[...TEXT_PRESETS.slice(0, 4), ...CAPTION_STYLES.slice(1, 3)].map((p) => (
            <button
              key={p.name}
              type="button"
              title={`Apply ${p.name} style`}
              onClick={() => edit({ type: "updateClip", clipId: clip.id, style: p.style })}
              className="flex h-9 items-center justify-center overflow-hidden rounded-md border border-line bg-[#2b3036] hover:border-accent"
            >
              <PresetSwatch style={{ ...p.style }} label="Aa" />
            </button>
          ))}
        </div>
      </Section>
      <Section title="Style">
        <Slider label="Size" value={style.fontSize} min={12} max={300} step={1} format={(v) => String(Math.round(v))} onChange={(v) => setStyle({ fontSize: v }, "size")} />
        <ColorInput label="Color" value={style.color} onChange={(v) => setStyle({ color: v }, "color")} />
        <label className="flex items-center justify-between">
          <span className="text-[12px] text-muted">Bold</span>
          <input type="checkbox" checked={style.bold} onChange={(e) => setStyle({ bold: e.target.checked }, "bold")} className="h-4 w-4 accent-[var(--color-accent)]" />
        </label>
        <Slider label="Outline" value={style.strokeWidth} min={0} max={20} step={0.5} format={(v) => v.toFixed(1)} onChange={(v) => setStyle({ strokeWidth: v }, "stroke")} />
        {style.strokeWidth > 0 && <ColorInput label="Outline color" value={style.strokeColor} onChange={(v) => setStyle({ strokeColor: v }, "strokeColor")} />}
        <label className="flex items-center justify-between">
          <span className="text-[12px] text-muted">Background box</span>
          <input
            type="checkbox"
            checked={style.background !== null}
            onChange={(e) => setStyle({ background: e.target.checked ? "#000000b3" : null }, "bg")}
            className="h-4 w-4 accent-[var(--color-accent)]"
          />
        </label>
        {style.background !== null && <ColorInput label="Box color" value={style.background} onChange={(v) => setStyle({ background: v }, "bgColor")} />}
      </Section>
    </>
  );
}

function ProjectSettings() {
  const project = useEditor((s) => s.snap!.project);
  const { edit } = useEditor.getState();
  return (
    <>
      <div className="flex flex-col items-center gap-2 px-6 py-8 text-center">
        <SlidersHorizontal size={22} className="text-subtle" />
        <p className="text-[13px] text-fg">Select a clip to edit it</p>
        <p className="text-[12px] text-muted">Click a clip on the timeline to change its size, position, volume or text.</p>
      </div>
      <Section title="Project">
        <div className="grid grid-cols-2 gap-1.5">
          {FORMATS.map((f) => {
            const active = f.width === project.canvas.width && f.height === project.canvas.height;
            return (
              <button
                key={f.label}
                type="button"
                aria-pressed={active}
                onClick={() => edit({ type: "setCanvas", width: f.width, height: f.height })}
                className={`flex flex-col items-start rounded-md border px-2.5 py-2 text-left ${active ? "border-accent bg-accent/10" : "border-line hover:border-muted"}`}
              >
                <span className="text-[13px] font-medium text-fg">{f.label}</span>
                <span className="text-[11px] text-muted">{f.hint}</span>
              </button>
            );
          })}
        </div>
        <ColorInput label="Background" value={project.canvas.background} onChange={(v) => edit({ type: "setCanvas", width: project.canvas.width, height: project.canvas.height, background: v }, "canvas-bg")} />
        <div className="tabular text-[12px] text-subtle">
          {project.canvas.width}×{project.canvas.height} · {project.canvas.fps} fps
        </div>
      </Section>
    </>
  );
}

export function Inspector() {
  const project = useEditor((s) => s.snap?.project);
  const selection = useEditor((s) => s.selection);
  const found = project && selection.length === 1 ? findClip(project, selection[0]) : null;

  let body;
  if (!project) body = null;
  else if (selection.length > 1)
    body = (
      <div className="px-6 py-8 text-center text-[13px] text-muted">
        {selection.length} clips selected. Press <span className="text-fg">Delete</span> to remove them.
      </div>
    );
  else if (!found) body = <ProjectSettings />;
  else {
    const { clip } = found;
    const c = clip.content;
    const asset = c.type === "media" ? project.assets.find((a) => a.id === c.assetId) : undefined;
    body = (
      <>
        <div className="border-b border-line px-4 py-3">
          <div className="truncate text-[13px] font-medium text-fg">{c.type === "text" ? "Text" : asset?.name}</div>
          <div className="tabular text-[12px] text-muted">
            {formatDuration(clip.durationUs)}
            {asset && asset.kind !== "image" && c.type === "media" && ` · from ${formatDuration(c.sourceInUs)} of ${formatDuration(asset.durationUs)}`}
          </div>
        </div>
        {c.type === "text" && <TextSection clip={clip} text={c.text} style={c.style} />}
        {c.type === "media" && asset?.hasAudio && asset.kind !== "image" && (
          <Section title="Audio">
            <Slider
              label="Volume"
              value={c.volume * 100}
              min={0}
              max={200}
              step={1}
              unit="%"
              format={(v) => String(Math.round(v))}
              onChange={(v) => useEditor.getState().edit({ type: "updateClip", clipId: clip.id, volume: v / 100 }, `${clip.id}:volume`)}
            />
          </Section>
        )}
        {(c.type === "text" || asset?.kind !== "audio") && <TransformSection clip={clip} transform={c.transform} asset={asset} />}
      </>
    );
  }

  return (
    <aside className="flex w-[300px] shrink-0 flex-col overflow-y-auto border-l border-line bg-panel" aria-label="Inspector">
      {body}
    </aside>
  );
}
