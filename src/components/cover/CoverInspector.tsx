import { useState } from "react";
import { AlertTriangle, Copy, Image as ImageIcon, RotateCcw, Trash2, Type } from "lucide-react";
import { COVER_PRESETS, addText, coverFormat, deleteText, downloadModels, editCover, editText, presetText, startFrom, thumbnailOf, useCover } from "../../lib/cover";
import { DEFAULT_TRANSFORM, sameStyle } from "../../lib/presets";
import { undoAction, useEditor } from "../../lib/store";
import { formatTime } from "../../lib/time";
import type { TextStyle, Thumbnail, ThumbnailFormat } from "../../lib/types";
import { QUIET, InspectorHeader } from "../inspector/Header";
import { StyleSection } from "../inspector/TextSection";
import { AiLock, Button, Checkbox, ColorInput, IconButton, PresetTile, Section, Segmented, Slider, TabBar, TabPanel, TextSwatch, useLockReason } from "../ui";
import { COVERED_TOO_MUCH, useShownCover } from "./CoverStage";

/** Why a setting that cuts the person out cannot be turned on, or null when it can. */
function useMaskReason(): string | null {
  const models = useCover((s) => s.models);
  if (models?.unavailable) return "Cutting out the person doesn't work on this computer";
  if (models && !models.downloaded) return `Download the cover models first (${models.sizeMb} MB)`;
  return null;
}

/** The inspector in the cover editor: the selected text, or the cover's own settings. */
export function CoverInspector({ format }: { format: ThumbnailFormat }) {
  const selected = useCover((s) => s.selected);
  const shown = useShownCover(format);
  if (!shown) return null;
  const text = typeof selected === "number" ? shown.cover.texts[selected] : undefined;
  return <AiLock>{text && !shown.draft && typeof selected === "number" ? <TextInspector format={format} index={selected} /> : <CoverSettings format={format} cover={shown.cover} draft={shown.draft} />}</AiLock>;
}

function CoverSettings({ format, cover, draft }: { format: ThumbnailFormat; cover: Thumbnail; draft: boolean }) {
  const f = coverFormat(format);
  const other = useEditor((s) => thumbnailOf(s.snap?.project, format === "cover_9x16" ? "youtube_16x9" : "cover_9x16"));
  const hidden = useCoverHidden();
  const maskReason = useMaskReason();
  const models = useCover((s) => s.models);
  const lock = useLockReason();
  const title = format === "cover_9x16" ? "Cover" : "YouTube thumbnail";
  // A change of the draft makes the cover from it.
  const set = (patch: Partial<Thumbnail>, key: string) => editCover(format, (c) => ({ ...(c ?? cover), ...patch }), key);
  const frame = (patch: Partial<Thumbnail["frame"]>, key: string) => editCover(format, (c) => ({ ...(c ?? cover), frame: { ...(c ?? cover).frame, ...patch } }), key);
  const background = (patch: Partial<Thumbnail["background"]>, key: string) =>
    editCover(format, (c) => ({ ...(c ?? cover), background: { ...(c ?? cover).background, ...patch } }), key);
  const removeCover = async () => {
    const snap = await useEditor.getState().edit({ type: "removeThumbnail", format });
    if (snap) useEditor.getState().toast({ kind: "info", text: `${title} removed`, action: undoAction(snap) });
  };
  const b = cover.background;

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="cover-inspector">
      <InspectorHeader icon={ImageIcon} title={title} detail={draft ? `${f.width}×${f.height} · Not made yet` : `${f.width}×${f.height} · Frame ${formatTime(cover.timeUs)}`} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {draft && other && (
          <div className="px-4 pb-1">
            <Button className="w-full" onClick={() => void startFrom(format, other)}>
              Start from the {other.format === "cover_9x16" ? "9:16 cover" : "YouTube thumbnail"}
            </Button>
          </div>
        )}
        <Section title="Text">
          <div className="grid grid-cols-3 gap-2">
            {COVER_PRESETS.map((p) => (
              <PresetTile key={p.name} label={p.name} selected={false} title={`Add a text in the ${p.name} style`} onClick={() => void addText(format, p)}>
                <PresetLook preset={p} />
              </PresetTile>
            ))}
          </div>
          {cover.texts.length > 0 && (
            <ul className="-mx-2 flex flex-col" aria-label="Texts on the cover">
              {cover.texts.map((t, i) => (
                <li key={i}>
                  <button
                    type="button"
                    onClick={() => useCover.setState({ selected: i })}
                    className="flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-[13px] text-fg hover:bg-white/[.08]"
                    title="Select this text"
                  >
                    <Type size={14} className="shrink-0 text-muted" />
                    <span className="min-w-0 flex-1 truncate">{t.text.replace(/\s+/g, " ") || "Empty text"}</span>
                    {t.behind && <span className="shrink-0 text-[11px] text-muted">Behind person</span>}
                    {(hidden[i] ?? 0) > COVERED_TOO_MUCH && <AlertTriangle size={14} className="shrink-0 text-warn" aria-label="The person covers too much of it" />}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </Section>
        <Section title="Person">
          {maskReason && !lock && !models?.unavailable && (
            <div className="flex items-center justify-between gap-2 text-[12px] text-muted">
              <span>Cutting out the person needs the cover models.</span>
              <Button className="h-6 shrink-0 px-2 text-[12px]" onClick={() => void downloadModels()}>
                Download {models?.sizeMb} MB
              </Button>
            </div>
          )}
          <Checkbox
            label="Outline"
            checked={!!cover.outline}
            disabled={!cover.outline && !!maskReason}
            title={!cover.outline && maskReason ? maskReason : undefined}
            onChange={(on) => void set({ outline: on ? { color: "#ffffff", width: format === "youtube_16x9" ? 10 : 14 } : null }, "outline")}
          />
          {cover.outline && (
            <>
              <ColorInput label="Outline color" value={cover.outline.color} onChange={(color) => void set({ outline: { ...cover.outline!, color } }, "outlineColor")} />
              <Slider label="Outline width" value={cover.outline.width} min={1} max={40} step={1} unit="px" format={(v) => String(Math.round(v))} onChange={(width) => void set({ outline: { ...cover.outline!, width } }, "outlineWidth")} />
            </>
          )}
        </Section>
        <Section title="Background">
          <Segmented
            label="Background"
            value={b.picture ? "picture" : "color"}
            disabled={b.picture && !!maskReason}
            disabledReason={maskReason ?? undefined}
            onChange={(id) => void background({ picture: id === "picture" }, "picture")}
            options={[
              { id: "picture", label: "Picture" },
              { id: "color", label: "Color" },
            ]}
          />
          {b.picture ? (
            <>
              <Slider label="Blur" value={b.blur * 100} min={0} max={100} step={1} unit="%" disabled={b.blur === 0 && !!maskReason} title={b.blur === 0 && maskReason ? maskReason : undefined} format={(v) => String(Math.round(v))} onChange={(v) => void background({ blur: v / 100 }, "blur")} />
              <Slider label="Dim" value={b.dim * 100} min={0} max={100} step={1} unit="%" disabled={b.dim === 0 && !!maskReason} title={b.dim === 0 && maskReason ? maskReason : undefined} format={(v) => String(Math.round(v))} onChange={(v) => void background({ dim: v / 100 }, "dim")} />
            </>
          ) : (
            <ColorInput label="Color" value={b.color} onChange={(color) => void background({ color }, "color")} />
          )}
        </Section>
        <Section
          title="Frame"
          actions={
            <IconButton label="Reset the frame" className={QUIET} onClick={() => void frame(DEFAULT_TRANSFORM, "reset")}>
              <RotateCcw size={14} />
            </IconButton>
          }
        >
          <Slider label="Scale" value={cover.frame.scale * 100} min={10} max={400} step={1} unit="%" format={(v) => String(Math.round(v))} onChange={(v) => void frame({ scale: v / 100 }, "scale")} />
          <Slider label="Position X" value={cover.frame.x * 100} min={-100} max={100} step={0.5} unit="%" format={(v) => v.toFixed(1)} onChange={(v) => void frame({ x: v / 100 }, "x")} />
          <Slider label="Position Y" value={cover.frame.y * 100} min={-100} max={100} step={0.5} unit="%" format={(v) => v.toFixed(1)} onChange={(v) => void frame({ y: v / 100 }, "y")} />
          <Slider label="Rotation" value={cover.frame.rotation} min={-180} max={180} step={1} unit="°" format={(v) => String(Math.round(v))} onChange={(v) => void frame({ rotation: v }, "rot")} />
        </Section>
        {!draft && (
          <div className="px-4 py-3">
            <Button variant="ghost" className="h-7 px-2 text-[12px]" onClick={() => void removeCover()}>
              <Trash2 size={13} /> Remove {format === "cover_9x16" ? "cover" : "thumbnail"}
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}

/** The share of each text of the open cover the person covers, from the cover on screen. */
function useCoverHidden(): number[] {
  return useCover((s) => s.hidden);
}

function TextInspector({ format, index }: { format: ThumbnailFormat; index: number }) {
  const [tab, setTab] = useState<"text" | "transform">("text");
  const text = useEditor((s) => thumbnailOf(s.snap?.project, format)?.texts[index]);
  const hidden = useCoverHidden()[index] ?? 0;
  const maskReason = useMaskReason();
  if (!text) return null;
  const setStyle = (patch: Partial<TextStyle>, key: string) => void editText(format, index, (t) => ({ ...t, style: { ...t.style, ...patch } }), `style:${key}`);
  const setTransform = (patch: Partial<Thumbnail["frame"]>, key: string) => void editText(format, index, (t) => ({ ...t, transform: { ...t.transform, ...patch } }), `transform:${key}`);
  const remove = () => deleteText(format, index);
  const duplicate = async () => {
    const snap = await editCover(format, (c) => (c ? { ...c, texts: [...c.texts, { ...c.texts[index], transform: { ...c.texts[index].transform, y: c.texts[index].transform.y + 0.06 } }] } : null));
    const count = thumbnailOf(snap?.project, format)?.texts.length ?? 0;
    if (snap) useCover.setState({ selected: count - 1 });
  };
  const t = text.transform;
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="cover-text-inspector">
      <div className="flex items-start justify-between pr-2">
        <InspectorHeader icon={Type} title="Text" detail={`On the ${format === "cover_9x16" ? "9:16 cover" : "YouTube thumbnail"}`} />
        <div className="flex items-center gap-0.5 pt-3">
          <IconButton label="Duplicate text" onClick={() => void duplicate()}>
            <Copy size={15} />
          </IconButton>
          <IconButton label="Delete text (Delete)" onClick={() => void remove()}>
            <Trash2 size={15} />
          </IconButton>
        </div>
      </div>
      <TabBar group="cover-text" label="Text settings" tabs={[{ id: "text", label: "Text" }, { id: "transform", label: "Transform" }]} value={tab} onChange={setTab} />
      <TabPanel group="cover-text" id={tab} className="min-h-0 flex-1 overflow-y-auto">
        {tab === "text" ? (
          <>
            <Section title="Text">
              <CoverTextField format={format} index={index} text={text.text} />
              <div className="grid grid-cols-3 gap-2">
                {COVER_PRESETS.map((p) => {
                  const look = presetText(format, p).style;
                  return (
                    <PresetTile
                      key={p.name}
                      label={p.name}
                      selected={sameStyle(look, text.style) && p.behind === text.behind}
                      title={`Apply the ${p.name} style`}
                      // A preset keeps the text's font and where it wraps, as text presets do.
                      onClick={() => void editText(format, index, (x) => ({ ...x, behind: p.behind && !maskReason ? true : x.behind && p.behind, style: { ...look, fontFamily: x.style.fontFamily, maxWidth: x.style.maxWidth } }))}
                    >
                      <PresetLook preset={p} />
                    </PresetTile>
                  );
                })}
              </div>
              <Checkbox
                label="Behind person"
                checked={text.behind}
                disabled={!text.behind && !!maskReason}
                title={!text.behind && maskReason ? maskReason : undefined}
                onChange={(behind) => void editText(format, index, (x) => ({ ...x, behind }), "behind")}
              />
              {text.behind && hidden > COVERED_TOO_MUCH && (
                <p className="flex items-start gap-1.5 text-[12px] text-fg" role="status">
                  <AlertTriangle size={14} className="mt-px shrink-0 text-warn" />
                  The person covers {Math.round(hidden * 100)} % of this text. Move it up or make it smaller.
                </p>
              )}
            </Section>
            <StyleSection style={text.style} setStyle={setStyle} maxSize={format === "cover_9x16" ? 400 : 300} />
          </>
        ) : (
          <Section
            title="Transform"
            actions={
              <IconButton label="Reset transform" className={QUIET} onClick={() => setTransform(DEFAULT_TRANSFORM, "reset")}>
                <RotateCcw size={14} />
              </IconButton>
            }
          >
            <Slider label="Scale" value={t.scale * 100} min={10} max={400} step={1} unit="%" format={(v) => String(Math.round(v))} onChange={(v) => setTransform({ scale: v / 100 }, "scale")} />
            <Slider label="Position X" value={t.x * 100} min={-100} max={100} step={0.5} unit="%" format={(v) => v.toFixed(1)} onChange={(v) => setTransform({ x: v / 100 }, "x")} />
            <Slider label="Position Y" value={t.y * 100} min={-100} max={100} step={0.5} unit="%" format={(v) => v.toFixed(1)} onChange={(v) => setTransform({ y: v / 100 }, "y")} />
            <Slider label="Rotation" value={t.rotation} min={-180} max={180} step={1} unit="°" format={(v) => String(Math.round(v))} onChange={(v) => setTransform({ rotation: v }, "rot")} />
            <Slider label="Opacity" value={t.opacity * 100} min={0} max={100} step={1} unit="%" format={(v) => String(Math.round(v))} onChange={(v) => setTransform({ opacity: v / 100 }, "opacity")} />
          </Section>
        )}
      </TabPanel>
    </div>
  );
}

/**
 * The text of a cover text. While it has focus the field shows what was typed, not the confirmed project,
 * which arrives a moment later and would move the caret.
 */
function CoverTextField({ format, index, text }: { format: ThumbnailFormat; index: number; text: string }) {
  const [draft, setDraft] = useState<string | null>(null);
  const lock = useLockReason();
  return (
    <textarea
      aria-label="Text"
      data-cover-text
      disabled={!!lock}
      title={lock ?? undefined}
      value={draft ?? text}
      rows={2}
      onChange={(e) => {
        setDraft(e.target.value);
        void editText(format, index, (t) => ({ ...t, text: e.target.value }), "text");
      }}
      onBlur={() => setDraft(null)}
      className="resize-y rounded-lg border border-white/[.08] bg-white/[.055] px-2.5 py-2 text-[13px] leading-[18px] text-fg outline-offset-0 focus:border-accent disabled:opacity-40"
    />
  );
}

/** A cover look's tile: its "Aa", and for a look behind the person a head and shoulders over the lower half of it. */
function PresetLook({ preset }: { preset: (typeof COVER_PRESETS)[number] }) {
  return (
    <span className="absolute inset-0 flex items-center justify-center overflow-hidden bg-line">
      <span className={preset.behind ? "-translate-y-1.5" : undefined}>
        <TextSwatch style={preset.style} label="Aa" size={preset.behind ? 22 : 17} />
      </span>
      {preset.behind && (
        <svg aria-hidden viewBox="0 0 40 30" className="absolute bottom-0 left-1/2 h-[62%] -translate-x-1/2 text-[#5c5c63]">
          <circle cx="20" cy="11" r="7.5" fill="currentColor" />
          <path d="M3 30c0-8 7.5-12 17-12s17 4 17 12z" fill="currentColor" />
        </svg>
      )}
    </span>
  );
}
