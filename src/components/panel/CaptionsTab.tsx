import { Captions, Check, Ellipsis, Plus } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { api, errorText, plainError } from "../../lib/api";
import { CAPTION_STYLES, KEYWORD_COLOR, KEYWORD_PICKS, isLook } from "../../lib/presets";
import { type CaptionLook, speechBlocker, startSpeech, useSpeech } from "../../lib/speech";
import { applyCaptionLook, isCaptionTrack, restyleCaptions, useEditor } from "../../lib/store";
import type { CaptionPreset, KeywordPick, TextStyle } from "../../lib/types";
import { FontPicker } from "../FontPicker";
import { Button, ColorInput, IconButton, Menu, Segmented, TextSwatch, useLockReason } from "../ui";
import { JobError, SpeechFields, SpeechJobCard, useSpeechJobs } from "./SpeechControls";

const WORDS = [
  { id: 1, label: "1", title: "One word at a time" },
  { id: 2, label: "2", title: "Up to two words, at most 15 characters, like reels" },
  { id: 3, label: "3", title: "Up to three words, at most 15 characters" },
  { id: 0, label: "Phrases", title: "Whole phrases" },
];

const BUSY = "Wait for speech recognition to finish";

/** The look of the captions on the timeline (the selected caption's, else the first one's), or null without captions. */
function useCaptionLook(): CaptionLook | null {
  const clip = useEditor((s) => {
    const clips = s.snap?.project.tracks.find(isCaptionTrack)?.clips;
    return clips?.find((c) => s.selection.includes(c.id)) ?? clips?.[0] ?? null;
  });
  if (clip?.content.type !== "text") return null;
  return { style: clip.content.style, animIn: clip.animIn ?? null, animOut: clip.animOut ?? null };
}

/** The user's own caption styles, read once per visit to the tab. */
function useMyStyles() {
  const [styles, setStyles] = useState<CaptionPreset[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.captionStyles().then(setStyles, (e) => setError(plainError(errorText(e))));
  }, []);
  /** Runs a change and shows the list it returns; resolves to whether it worked. */
  const change = async (run: () => Promise<CaptionPreset[]>) => {
    try {
      setStyles(await run());
      setError(null);
      return true;
    } catch (e) {
      useEditor.getState().toast({ kind: "error", text: plainError(errorText(e)) });
      return false;
    }
  };
  return { styles, error, change };
}

export function CaptionsTab() {
  const lock = useLockReason();
  const captionWords = useSpeech((s) => s.captionWords);
  const pending = useSpeech((s) => s.captionLook);
  const onTimeline = useCaptionLook();
  const { running, last } = useSpeechJobs("captions");
  const blocker = useEditor((s) => (s.snap ? speechBlocker(s.snap.project) : null));
  const hasCaptions = onTimeline !== null;
  // With captions on the timeline, the highlighted style and the font are the ones they use.
  const look = onTimeline ?? pending;
  const font = look.style.fontFamily ?? null;
  const mine = useMyStyles();
  const busy = !!running;

  // Regenerating keeps the look of the captions already there, edits included.
  const start = () => startSpeech(look);
  const apply = (preset: CaptionPreset) => {
    const next: CaptionLook = { style: { ...preset.style, fontFamily: preset.style.fontFamily ?? font }, animIn: preset.animIn ?? null, animOut: preset.animOut ?? null };
    if (hasCaptions) applyCaptionLook(next);
    else useSpeech.setState({ captionLook: next });
  };
  const restyle = (patch: Partial<TextStyle>) => (hasCaptions ? restyleCaptions(patch) : useSpeech.setState({ captionLook: { ...pending, style: { ...pending.style, ...patch } } }));
  const pick = look.style.keywords?.pick ?? "off";
  const setPick = (id: KeywordPick | "off") => restyle({ keywords: id === "off" ? null : { color: look.style.keywords?.color ?? KEYWORD_COLOR, pick: id } });

  const tile: Tile = (preset, own) => {
    const current = isLook(preset, look.style, look.animIn, look.animOut);
    const disabled = busy || !!lock;
    return (
      <div key={preset.name} className="group relative min-w-0">
        <button
          type="button"
          aria-pressed={current}
          aria-disabled={disabled || undefined}
          title={lock ?? (busy ? BUSY : hasCaptions ? `Apply ${preset.name} to all captions` : `Use ${preset.name} for new captions`)}
          onClick={disabled ? undefined : () => apply(preset)}
          onKeyDown={(e) => {
            if (!own || disabled) return;
            if (e.key === "F2") {
              e.preventDefault();
              own.rename();
            } else if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
              e.preventDefault();
              const r = e.currentTarget.getBoundingClientRect();
              own.menu({ x: r.left, y: r.bottom + 4 }, true);
            }
          }}
          className={`relative flex h-11 w-full min-w-0 items-center justify-center rounded-[10px] border bg-white/[.1] px-1 transition-colors duration-[120ms] ease-out aria-disabled:cursor-not-allowed aria-disabled:opacity-40 ${
            current ? "border-accent shadow-[0_0_0_1px_var(--color-accent)]" : disabled ? "border-white/[.08]" : "border-white/[.08] hover:border-white/30"
          }`}
        >
          <TextSwatch style={{ ...preset.style, fontFamily: preset.style.fontFamily ?? font }} label={preset.name} size={12} />
          {current && (
            <span aria-hidden className="absolute top-1 right-1 flex h-3.5 w-3.5 items-center justify-center rounded-full bg-accent-strong text-white shadow-[0_1px_2px_rgb(0_0_0/.5)]">
              <Check size={10} strokeWidth={3} />
            </span>
          )}
        </button>
        {own && !disabled && (
          <button
            type="button"
            tabIndex={-1}
            aria-hidden
            title="More"
            onClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              own.menu({ x: r.left, y: r.bottom + 4 }, false);
            }}
            className="absolute top-1 left-1 flex h-5 w-5 items-center justify-center rounded-full bg-black/55 text-fg opacity-0 transition-opacity duration-[120ms] group-hover:opacity-100 hover:bg-black/75"
          >
            <Ellipsis size={12} />
          </button>
        )}
      </div>
    );
  };

  return (
    <div className="flex flex-col gap-3.5 overflow-y-auto p-3.5">
      <SpeechFields disabled={busy} />
      <div className="flex flex-col gap-1.5">
        <span className="text-[12px] text-muted">{hasCaptions ? "Words per caption · applies when you regenerate" : "Words per caption"}</span>
        <Segmented label="Words per caption" value={captionWords} onChange={(n) => useSpeech.setState({ captionWords: n })} options={WORDS} disabled={busy} disabledReason={BUSY} />
      </div>
      <div className="flex flex-col gap-1.5">
        <span className="text-[12px] text-muted">Style</span>
        <div className="grid grid-cols-[repeat(auto-fill,minmax(72px,1fr))] gap-2">{CAPTION_STYLES.map((s) => tile(s))}</div>
      </div>
      <MyStyles look={look} mine={mine} busy={busy} tile={tile} />
      <div className="flex flex-col gap-1.5">
        <span className="text-[12px] text-muted">Key words</span>
        <Segmented label="Key words" value={pick} onChange={setPick} options={KEYWORD_PICKS} disabled={busy} disabledReason={BUSY} />
        {look.style.keywords && <ColorInput label="Key word color" value={look.style.keywords.color} onChange={(color) => restyle({ keywords: { ...look.style.keywords!, color } })} />}
      </div>
      <div className="flex flex-col gap-1.5">
        <span className="text-[12px] text-muted">Font</span>
        {/* A row, so the picker's flex-1 fills the width instead of collapsing the column. */}
        <div className="flex">
          <FontPicker value={font} onChange={(fontFamily) => restyle({ fontFamily })} disabled={busy} disabledReason={BUSY} />
        </div>
      </div>
      {running ? (
        <SpeechJobCard job={running} />
      ) : (
        <span className="flex">
          <Button variant="primary" className="flex-1" disabled={!!blocker} disabledReason={blocker ?? undefined} onClick={start}>
            <Captions size={15} /> {hasCaptions ? "Regenerate captions" : "Generate captions"}
          </Button>
        </span>
      )}
      {!running && <JobError job={last} />}
    </div>
  );
}

/** A style tile; the user's own get a menu (… on hover, the context menu key) and F2 to rename. */
type Tile = (preset: CaptionPreset, own?: { menu: (at: { x: number; y: number }, keyboard: boolean) => void; rename: () => void }) => ReactNode;

/** The user's own styles: save the current look under a name, apply, rename and delete. */
function MyStyles({ look, mine, busy, tile }: { look: CaptionLook; mine: ReturnType<typeof useMyStyles>; busy: boolean; tile: Tile }) {
  const lock = useLockReason();
  const [naming, setNaming] = useState<{ from: string | null } | null>(null);
  const [menu, setMenu] = useState<{ name: string; at: { x: number; y: number }; keyboard: boolean } | null>(null);
  const unique = (base: string) => {
    for (let n = 1; ; n++) if (!mine.styles.some((s) => s.name.toLowerCase() === `${base} ${n}`.toLowerCase())) return `${base} ${n}`;
  };
  const save = async (name: string) => {
    const from = naming?.from;
    const done = from
      ? from === name || (await mine.change(() => api.renameCaptionStyle(from, name)))
      : await mine.change(() => api.saveCaptionStyle({ name, style: look.style, animIn: look.animIn, animOut: look.animOut }));
    if (done) setNaming(null);
  };
  const remove = async (name: string) => {
    const style = mine.styles.find((s) => s.name === name);
    if (!style || !(await mine.change(() => api.deleteCaptionStyle(name)))) return;
    useEditor.getState().toast({ kind: "info", text: `Deleted “${name}”`, action: { label: "Undo", run: () => void mine.change(() => api.saveCaptionStyle(style)) } });
  };

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex min-h-6 items-center justify-between gap-2">
        <span className="text-[12px] text-muted">My styles</span>
        <IconButton label={busy ? BUSY : "Save the current look as a style"} disabled={busy || !!naming} onClick={() => setNaming({ from: null })} className="h-6 w-6">
          <Plus size={14} />
        </IconButton>
      </div>
      {naming && <NameField initial={naming.from ?? unique("My style")} onSave={save} onCancel={() => setNaming(null)} />}
      {mine.error && <p className="text-[12px] text-danger">{mine.error}</p>}
      {mine.styles.length > 0 && (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(72px,1fr))] gap-2">
          {mine.styles.map((s) => tile(s, { menu: (at, keyboard) => setMenu({ name: s.name, at, keyboard }), rename: () => setNaming({ from: s.name }) }))}
        </div>
      )}
      {menu && (
        <Menu
          label={`${menu.name} style`}
          at={menu.at}
          keyboard={menu.keyboard}
          onClose={() => setMenu(null)}
          items={[
            { label: "Rename", shortcut: "F2", disabled: lock, run: () => setNaming({ from: menu.name }) },
            { label: "Delete", danger: true, disabled: lock, run: () => void remove(menu.name) },
          ]}
        />
      )}
    </div>
  );
}

/** A name typed in place: Enter saves, Esc or leaving it empty cancels. */
function NameField({ initial, onSave, onCancel }: { initial: string; onSave: (name: string) => void; onCancel: () => void }) {
  const [name, setName] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => ref.current?.select(), []);
  const submit = () => (name.trim() ? onSave(name.trim()) : onCancel());
  return (
    <div className="flex items-center gap-1.5">
      <input
        ref={ref}
        aria-label="Style name"
        value={name}
        maxLength={40}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") submit();
          else if (e.key === "Escape") onCancel();
        }}
        className="h-8 min-w-0 flex-1 rounded-lg border border-white/[.08] bg-white/[.055] px-2.5 text-[13px] text-fg outline-none focus:border-accent"
      />
      <Button onClick={submit}>Save</Button>
      <Button variant="ghost" onClick={onCancel}>
        Cancel
      </Button>
    </div>
  );
}
