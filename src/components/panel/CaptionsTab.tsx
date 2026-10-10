import { Captions, Check, Ellipsis, Plus } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { api, errorText, plainError } from "../../lib/api";
import { CAPTION_STYLES, KEYWORD_COLOR, KEYWORD_PICKS, isLook } from "../../lib/presets";
import { type CaptionLook, speechBlocker, startSpeech, useSpeech } from "../../lib/speech";
import { applyCaptionLook, isCaptionTrack, restyleCaptions, useEditor } from "../../lib/store";
import type { CaptionPreset, KeywordPick, TextStyle } from "../../lib/types";
import { FontPicker } from "../FontPicker";
import { Button, ColorInput, IconButton, Menu, Segmented, TextSwatch, lockedProps, useLockReason } from "../ui";
import { JobError, SpeechFields, SpeechJobCard, useSpeechJobs } from "./SpeechControls";

const WORDS = [
  { id: 1, label: "1", title: "One word at a time" },
  { id: 2, label: "2", title: "Up to two words, at most 15 characters, like reels" },
  { id: 3, label: "3", title: "Up to three words, at most 15 characters" },
  { id: 0, label: "Phrases", title: "Whole phrases" },
];

const BUSY = "Wait for speech recognition to finish";
const GRID = "grid grid-cols-[repeat(auto-fill,minmax(72px,1fr))] gap-2";

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
  /** Runs a change and shows the list it returns; resolves to why it failed, or null. */
  const change = async (run: () => Promise<CaptionPreset[]>) => {
    try {
      setStyles(await run());
      setError(null);
      return null;
    } catch (e) {
      return plainError(errorText(e));
    }
  };
  return { styles, error, change };
}

/** A style tile; the user's own get a menu (…, right click, the context menu key), F2 to rename and a ref for focus. */
type Tile = (
  preset: CaptionPreset,
  own?: { menu: (at: { x: number; y: number }, keyboard: boolean) => void; rename: () => void; ref: (el: HTMLButtonElement | null) => void },
) => ReactNode;

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
    // The user's own styles are not the project: recognition running leaves renaming and deleting them alone.
    const openMenu = (at: { x: number; y: number }, keyboard: boolean) => !lock && own?.menu(at, keyboard);
    return (
      <div key={preset.name} className="group relative min-w-0">
        <button
          type="button"
          ref={own?.ref}
          aria-pressed={current}
          aria-disabled={disabled || undefined}
          title={lock ?? (busy ? BUSY : hasCaptions ? `Apply ${preset.name} to all captions` : `Use ${preset.name} for new captions`)}
          onClick={disabled ? undefined : () => apply(preset)}
          onContextMenu={(e) => {
            if (!own) return;
            e.preventDefault();
            openMenu({ x: e.clientX, y: e.clientY }, false);
          }}
          onKeyDown={(e) => {
            if (!own || lock) return;
            if (e.key === "F2") {
              e.preventDefault();
              own.rename();
            } else if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
              e.preventDefault();
              const r = e.currentTarget.getBoundingClientRect();
              openMenu({ x: r.left, y: r.bottom + 4 }, true);
            }
          }}
          className={`relative flex h-11 w-full min-w-0 items-center justify-center rounded-[10px] border bg-line transition-colors duration-[120ms] ease-out aria-disabled:cursor-not-allowed aria-disabled:opacity-40 ${own ? "px-5" : "px-1"} ${
            current ? "border-accent shadow-[0_0_0_1px_var(--color-accent)]" : disabled ? "border-white/[.08]" : "border-white/[.08] hover:border-white/30"
          }`}
        >
          <TextSwatch style={{ ...preset.style, fontFamily: preset.style.fontFamily ?? font }} label={preset.name} size={12} lines={2} />
          {current && (
            <span aria-hidden className="absolute top-1 right-1 flex h-3.5 w-3.5 items-center justify-center rounded-full bg-accent-strong text-white shadow-[0_1px_2px_rgb(0_0_0/.5)]">
              <Check size={10} strokeWidth={3} />
            </span>
          )}
        </button>
        {own && !lock && (
          <button
            type="button"
            tabIndex={-1}
            aria-hidden
            title="More"
            onClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              openMenu({ x: r.left, y: r.bottom + 4 }, false);
            }}
            className="absolute right-1 bottom-1 flex h-5 w-5 items-center justify-center rounded-full bg-black/55 text-fg opacity-0 transition-opacity duration-[120ms] group-hover:opacity-100 hover:bg-black/75"
          >
            <Ellipsis size={12} />
          </button>
        )}
      </div>
    );
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex min-h-0 flex-1 flex-col gap-3.5 overflow-y-auto p-3.5">
        <SpeechFields disabled={busy} />
        <div className="flex flex-col gap-1.5">
          <span className="text-[12px] text-muted">{hasCaptions ? "Words per caption · applies when you regenerate" : "Words per caption"}</span>
          <Segmented label="Words per caption" value={captionWords} onChange={(n) => useSpeech.setState({ captionWords: n })} options={WORDS} disabled={busy} disabledReason={BUSY} />
        </div>
        <div className="flex flex-col gap-1.5">
          <span className="text-[12px] text-muted">Style</span>
          <div className={GRID}>{CAPTION_STYLES.map((s) => tile(s))}</div>
        </div>
        <MyStyles look={look} mine={mine} tile={tile} />
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
      </div>
      {/* Below the choices, in view however many styles there are. */}
      <div className="flex shrink-0 flex-col gap-2 border-t border-white/[.07] p-3.5">
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
    </div>
  );
}

/** The user's own styles: save the current look under a name, apply, rename and delete. */
function MyStyles({ look, mine, tile }: { look: CaptionLook; mine: ReturnType<typeof useMyStyles>; tile: Tile }) {
  const lock = useLockReason();
  const [naming, setNaming] = useState<{ from: string | null; error?: string } | null>(null);
  const [menu, setMenu] = useState<{ name: string; at: { x: number; y: number }; keyboard: boolean } | null>(null);
  const add = useRef<HTMLButtonElement>(null);
  const tiles = useRef(new Map<string, HTMLButtonElement>());
  // Where focus goes once naming, a menu or deleting ends: a tile by name, else +.
  const [focus, setFocus] = useState<{ name: string | null } | null>(null);
  useEffect(() => {
    if (!focus) return;
    ((focus.name && tiles.current.get(focus.name)) || add.current)?.focus();
    setFocus(null);
  }, [focus]);
  const unique = (base: string) => {
    for (let n = 1; ; n++) if (!mine.styles.some((s) => s.name.toLowerCase() === `${base} ${n}`.toLowerCase())) return `${base} ${n}`;
  };
  const done = (name: string | null) => {
    setNaming(null);
    setFocus({ name });
  };
  const save = async (name: string) => {
    const from = naming?.from ?? null;
    if (lock || from === name) return done(from);
    const error = from ? await mine.change(() => api.renameCaptionStyle(from, name)) : await mine.change(() => api.saveCaptionStyle({ name, style: look.style, animIn: look.animIn, animOut: look.animOut }));
    if (error) setNaming({ from, error });
    else done(name);
  };
  const remove = async (name: string) => {
    const at = mine.styles.findIndex((s) => s.name === name);
    const style = mine.styles[at];
    if (!style) return;
    const error = await mine.change(() => api.deleteCaptionStyle(name));
    if (error) return useEditor.getState().toast({ kind: "error", text: error });
    const rest = mine.styles.filter((s) => s.name !== name);
    setFocus({ name: (rest[at] ?? rest[at - 1])?.name ?? null });
    useEditor.getState().toast({ kind: "info", text: `Deleted “${name}”`, action: { label: "Undo", run: () => void mine.change(() => api.saveCaptionStyle(style)) } });
  };

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex min-h-6 items-center justify-between gap-2">
        <span className="text-[12px] text-muted">My styles</span>
        <IconButton ref={add} label="Save the current look as a style" disabled={!!naming} onClick={() => setNaming({ from: null })} className="h-6 w-6">
          <Plus size={14} />
        </IconButton>
      </div>
      {naming && <NameField key={naming.from ?? ""} initial={naming.from ?? unique("My style")} error={naming.error} onSave={save} onCancel={() => done(naming.from)} />}
      {mine.error && <p className="text-[12px] text-danger">{mine.error}</p>}
      {mine.styles.length > 0 ? (
        <div className={GRID}>
          {mine.styles.map((s) =>
            tile(s, {
              menu: (at, keyboard) => setMenu({ name: s.name, at, keyboard }),
              rename: () => setNaming({ from: s.name }),
              ref: (el) => void (el ? tiles.current.set(s.name, el) : tiles.current.delete(s.name)),
            }),
          )}
        </div>
      ) : (
        !naming &&
        !mine.error && (
          <div className={GRID}>
            <button
              type="button"
              {...lockedProps(lock)}
              onClick={lock ? undefined : () => setNaming({ from: null })}
              className="flex h-11 min-w-0 items-center justify-center gap-1 rounded-[10px] border border-dashed border-white/[.15] text-[12px] text-muted transition-colors duration-[120ms] hover:border-white/30 hover:text-fg aria-disabled:cursor-not-allowed aria-disabled:opacity-40"
            >
              <Plus size={13} /> Save look
            </button>
          </div>
        )
      )}
      {menu && (
        <Menu
          label={`${menu.name} style`}
          at={menu.at}
          keyboard={menu.keyboard}
          onClose={(chose) => {
            if (!chose) setFocus({ name: menu.name });
            setMenu(null);
          }}
          items={[
            { label: "Rename", shortcut: "F2", run: () => setNaming({ from: menu.name }) },
            { label: "Delete", danger: true, run: () => void remove(menu.name) },
          ]}
        />
      )}
    </div>
  );
}

/** A name typed in place: Enter saves, Esc or leaving it empty cancels. A refused name says why under it. */
function NameField({ initial, error, onSave, onCancel }: { initial: string; error?: string; onSave: (name: string) => void; onCancel: () => void }) {
  const [name, setName] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => ref.current?.select(), []);
  useEffect(() => {
    if (error) ref.current?.focus();
  }, [error]);
  const submit = () => (name.trim() ? onSave(name.trim()) : onCancel());
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-1.5">
        <input
          ref={ref}
          aria-label="Style name"
          aria-invalid={!!error || undefined}
          value={name}
          maxLength={40}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") submit();
            else if (e.key === "Escape") onCancel();
          }}
          className={`h-8 min-w-0 flex-1 rounded-lg border bg-white/[.055] px-2.5 text-[13px] text-fg outline-none ${error ? "border-danger" : "border-white/[.08] focus:border-accent"}`}
        />
        <Button onClick={submit}>Save</Button>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
      {error && <p className="text-[12px] text-danger">{error}</p>}
    </div>
  );
}
