import { CAPTION_STYLES, TEXT_PRESETS, sameStyle } from "../../lib/presets";
import { useEditor } from "../../lib/store";
import type { Clip, TextStyle } from "../../lib/types";
import { Checkbox, ColorInput, Section, Slider, TextSwatch } from "../ui";

const STYLE_PRESETS = [...TEXT_PRESETS.slice(0, 4), ...CAPTION_STYLES.slice(1, 3)];

export function TextSection({ clip, text, style }: { clip: Clip; text: string; style: TextStyle }) {
  const edit = useEditor((s) => s.edit);
  const setStyle = (patch: Partial<TextStyle>, key: string) => edit({ type: "updateClip", clipId: clip.id, style: { ...style, ...patch } }, `${clip.id}:style:${key}`);
  return (
    <>
      <Section title="Text">
        <textarea
          aria-label="Text"
          value={text}
          rows={3}
          onChange={(e) => edit({ type: "updateClip", clipId: clip.id, text: e.target.value }, `${clip.id}:text`)}
          className="resize-y rounded-md border border-line bg-raised p-2 text-[13px] text-fg focus:border-accent"
        />
        <div className="grid grid-cols-3 gap-1.5">
          {STYLE_PRESETS.map((p) => {
            const on = sameStyle(p.style, style);
            return (
              <button
                key={p.name}
                type="button"
                aria-pressed={on}
                title={`Apply ${p.name} style`}
                onClick={() => edit({ type: "updateClip", clipId: clip.id, style: p.style })}
                className={`flex h-9 items-center justify-center overflow-hidden rounded-md border bg-[#2b3036] ${on ? "border-accent shadow-[0_0_0_1px_var(--color-accent)]" : "border-line hover:border-muted"}`}
              >
                <TextSwatch style={p.style} label="Aa" />
              </button>
            );
          })}
        </div>
      </Section>
      <Section title="Style">
        <Slider label="Size" value={style.fontSize} min={12} max={300} step={1} format={(v) => String(Math.round(v))} onChange={(v) => setStyle({ fontSize: v }, "size")} />
        <ColorInput label="Color" value={style.color} onChange={(v) => setStyle({ color: v }, "color")} />
        <Checkbox label="Bold" checked={style.bold} onChange={(v) => setStyle({ bold: v }, "bold")} />
        <Slider label="Outline" value={style.strokeWidth} min={0} max={20} step={0.5} format={(v) => v.toFixed(1)} onChange={(v) => setStyle({ strokeWidth: v }, "stroke")} />
        {style.strokeWidth > 0 && <ColorInput label="Outline color" value={style.strokeColor} onChange={(v) => setStyle({ strokeColor: v }, "strokeColor")} />}
        <Checkbox label="Background box" checked={style.background !== null} onChange={(v) => setStyle({ background: v ? "#000000b3" : null }, "bg")} />
        {style.background !== null && <ColorInput label="Box color" value={style.background} onChange={(v) => setStyle({ background: v }, "bgColor")} />}
      </Section>
    </>
  );
}
