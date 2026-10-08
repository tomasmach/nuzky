import { useState } from "react";
import { CAPTION_STYLES, TEXT_PRESETS, sameStyle } from "../../lib/presets";
import { editClip, useEditor } from "../../lib/store";
import type { Clip, TextStyle } from "../../lib/types";
import { FontPicker } from "../FontPicker";
import { Checkbox, ColorInput, PresetTile, Section, Slider, TextSwatch, useLockReason } from "../ui";

/** Label column of the inspector's property rows, next to a full-width control. */
export function FieldRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-[78px] shrink-0 truncate text-[12px] text-muted">{label}</span>
      {children}
    </div>
  );
}

/**
 * The clip's text. While it has focus the field shows what was typed, not the confirmed project,
 * which arrives a moment later and would move the caret or drop keys typed in between.
 */
function TextField({ clipId, text }: { clipId: string; text: string }) {
  const edit = useEditor((s) => s.edit);
  const [draft, setDraft] = useState<string | null>(null);
  const lock = useLockReason();
  return (
    <textarea
      aria-label="Text"
      disabled={!!lock}
      title={lock ?? undefined}
      value={draft ?? text}
      rows={3}
      onChange={(e) => {
        setDraft(e.target.value);
        edit({ type: "updateClip", clipId, text: e.target.value }, `${clipId}:text`);
      }}
      onBlur={() => setDraft(null)}
      className="resize-y rounded-lg border border-white/[.08] bg-white/[.055] px-2.5 py-2 text-[13px] leading-[18px] text-fg outline-offset-0 focus:border-accent disabled:opacity-40"
    />
  );
}

/**
 * Why no word of a karaoke caption lights up, or null when they do: the engine highlights words only
 * while the text is exactly them joined by single spaces.
 */
function unlitReason(clip: Clip, text: string) {
  const words = clip.content.type === "text" ? (clip.content.words ?? []) : [];
  if (words.length === 0) return "This caption has no word timing. Regenerate captions to add it.";
  if (words.map((w) => w.text).join(" ") !== text) return "Words were added or removed, so none lights up. Regenerate captions to bring it back.";
  return null;
}

/** Style tiles use the presets and names of the Text tab, or of the Captions tab for a caption. */
export function TextSection({ clip, text, style, caption }: { clip: Clip; text: string; style: TextStyle; caption: boolean }) {
  const edit = useEditor((s) => s.edit);
  // Built from the latest confirmed style, so two quick changes to different properties both stick.
  const setStyle = (patch: Partial<TextStyle>, key: string) =>
    editClip(clip.id, (c) => (c.content.type === "text" ? { type: "updateClip", clipId: c.id, style: { ...c.content.style, ...patch } } : null), `${clip.id}:style:${key}`);
  const presets = caption ? CAPTION_STYLES : TEXT_PRESETS;
  return (
    <>
      <Section title="Text">
        <TextField clipId={clip.id} text={text} />
        <div className="grid grid-cols-3 gap-2">
          {presets.map((p) => (
            <PresetTile key={p.name} label={p.name} selected={sameStyle(p.style, style)} title={`Apply ${p.name} style`} onClick={() => edit({ type: "updateClip", clipId: clip.id, style: { ...p.style, fontFamily: style.fontFamily, maxWidth: style.maxWidth } })}>
              <span className="absolute inset-0 flex items-center justify-center bg-line">
                <TextSwatch style={{ ...p.style, fontFamily: style.fontFamily }} label="Aa" />
              </span>
            </PresetTile>
          ))}
        </div>
      </Section>
      <Section title="Style">
        <FieldRow label="Font">
          <FontPicker value={style.fontFamily} onChange={(fontFamily) => setStyle({ fontFamily }, "font")} />
        </FieldRow>
        <Slider label="Size" value={style.fontSize} min={12} max={300} step={1} format={(v) => String(Math.round(v))} onChange={(v) => setStyle({ fontSize: v }, "size")} />
        <ColorInput label="Color" value={style.color} onChange={(v) => setStyle({ color: v }, "color")} />
        <Checkbox label="Bold" checked={style.bold} onChange={(v) => setStyle({ bold: v }, "bold")} />
        <Slider label="Outline" value={style.strokeWidth} min={0} max={20} step={0.5} format={(v) => v.toFixed(1)} onChange={(v) => setStyle({ strokeWidth: v }, "stroke")} />
        {style.strokeWidth > 0 && <ColorInput label="Outline color" value={style.strokeColor} onChange={(v) => setStyle({ strokeColor: v }, "strokeColor")} />}
        <Checkbox label="Background box" checked={style.background !== null} onChange={(v) => setStyle({ background: v ? "#000000b3" : null }, "bg")} />
        {style.background !== null && <ColorInput label="Box color" value={style.background} onChange={(v) => setStyle({ background: v }, "bgColor")} />}
      </Section>
      {caption && (
        <Section title="Karaoke">
          <Checkbox label="Highlight spoken word" checked={!!style.highlight} onChange={(v) => setStyle({ highlight: v ? "#ffe14d" : null }, "highlight")} />
          {style.highlight && (
            <>
              <ColorInput label="Highlight color" value={style.highlight} onChange={(v) => setStyle({ highlight: v }, "highlightColor")} />
              {unlitReason(clip, text) && <p className="text-[12px] text-muted">{unlitReason(clip, text)}</p>}
            </>
          )}
        </Section>
      )}
    </>
  );
}
