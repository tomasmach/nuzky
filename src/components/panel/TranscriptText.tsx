import { Fragment, memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { Trash2 } from "lucide-react";
import { followPointer } from "../../lib/drag";
import { MAX_WORD_CHARS, paragraphs, tokenAt, type Token } from "../../lib/speech";
import { aiLocked, useEditor } from "../../lib/store";
import { formatDuration, formatTime } from "../../lib/time";
import { Button, useLockReason } from "../ui";

interface Selection {
  anchor: number;
  focus: number;
}

const optionId = (i: number) => `transcript-token-${i}`;

/** The word being corrected, in place: Enter or leaving the field saves, Esc cancels. */
function WordField({ word, onDone }: { word: string; onDone: (text: string | null) => void }) {
  const [value, setValue] = useState(word);
  const done = useRef(false);
  const finish = (text: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(text);
  };
  return (
    <input
      autoFocus
      aria-label={`Correct “${word}”`}
      value={value}
      maxLength={MAX_WORD_CHARS}
      spellCheck={false}
      onChange={(e) => setValue(e.target.value)}
      onFocus={(e) => e.currentTarget.select()}
      onKeyDown={(e) => {
        // The field keeps its keys: arrows, Delete and Space edit the text, not the transcript.
        e.stopPropagation();
        if (e.key === "Enter" || e.key === "Escape") {
          e.preventDefault();
          finish(e.key === "Enter" ? value : null);
        }
      }}
      onBlur={() => finish(value)}
      style={{ width: `${Math.max(value.length, 2) + 1}ch` }}
      className="mx-0.5 rounded-sm bg-raised px-0.5 text-[13px] leading-[18px] text-fg"
    />
  );
}

/** One paragraph; re-renders only when the current word, the selection or the word being corrected inside it changes. */
const Paragraph = memo(function Paragraph({
  tokens,
  from,
  to,
  active,
  lo,
  hi,
  editing,
  onEdited,
}: {
  tokens: Token[];
  from: number;
  to: number;
  active: number;
  lo: number;
  hi: number;
  editing: number;
  onEdited: (text: string | null) => void;
}) {
  const items = [];
  for (let i = from; i < to; i++) {
    const t = tokens[i];
    const selected = i >= lo && i <= hi;
    const playing = i === active;
    // Accent text on the accent selection is hard to read, so there the word playing is black on accent.
    const look = selected ? (playing ? "bg-accent text-black" : "bg-accent/30 text-fg") : playing ? "text-accent" : "";
    items.push(
      <Fragment key={i}>
        {/* The space between two selected tokens is filled too, so the selection reads as one band. */}
        {i > from && (i > lo && i <= hi ? <span className="bg-accent/30"> </span> : " ")}
        {t.kind === "word" && i === editing ? (
          <WordField word={t.text} onDone={onEdited} />
        ) : t.kind === "word" ? (
          <span
            id={optionId(i)}
            data-t={i}
            role="option"
            aria-selected={selected}
            title={t.original ? `Recognised as “${t.original}”` : undefined}
            className={`cursor-pointer ${selected ? `${i === lo ? "rounded-l-sm" : ""} ${i === hi ? "rounded-r-sm" : ""}` : "rounded-sm hover:bg-raised"} ${t.original ? "underline decoration-muted decoration-dotted underline-offset-3" : ""} ${look}`}
          >
            {t.text}
          </span>
        ) : (
          <span
            id={optionId(i)}
            data-t={i}
            role="option"
            aria-selected={selected}
            aria-label={`Pause, ${formatDuration(t.gapUs)}`}
            title={`Pause of ${formatDuration(t.gapUs)}`}
            className={`tabular cursor-pointer whitespace-nowrap rounded px-1 py-px text-[11px] ${selected ? look : `bg-raised hover:bg-line ${playing ? "text-accent" : "text-muted"}`}`}
          >
            {formatDuration(t.gapUs)}
          </span>
        )}
      </Fragment>,
    );
  }
  return (
    <p className="mb-3 text-[13px] leading-[22px] text-fg/90">
      <span className="tabular mr-2 text-[11px] text-muted">{formatTime(tokens[from].startUs, false)}</span>
      {items}
    </p>
  );
});

/** Same words and pauses at the same places: only the text of a word changed, so indices still hold. */
const sameShape = (a: Token[], b: Token[]) => a.length === b.length && a.every((t, n) => t.kind === b[n].kind && t.startUs === b[n].startUs);

/**
 * The transcript as text. Click a word to jump there, drag or Shift-click to select a range,
 * Delete cuts it from the timeline unless `blocker` says why it cannot. ←/→ move word by word
 * (Shift extends), Esc clears. Double-click a word, or F2 with one word selected, to correct it.
 */
export function TranscriptText({
  tokens,
  blocker,
  onDelete,
  onCorrect,
}: {
  tokens: Token[];
  blocker: string | null;
  onDelete: (lo: number, hi: number) => Promise<boolean>;
  /** `i` is the word's index in the view. */
  onCorrect: (i: number, text: string, shown: string) => Promise<boolean>;
}) {
  const [sel, setSel] = useState<Selection | null>(null);
  const [editing, setEditing] = useState<number | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const lock = useLockReason();
  // Re-renders when the word playing changes, not on every frame.
  const active = useEditor((s) => tokenAt(tokens, s.timeUs));
  const playing = useEditor((s) => s.playing);
  const paras = useMemo(() => paragraphs(tokens), [tokens]);
  // Fewer tokens can arrive before the effect below clears the selection.
  const live = sel && sel.anchor < tokens.length && sel.focus < tokens.length ? sel : null;
  const lo = live ? Math.min(live.anchor, live.focus) : -1;
  const hi = live ? Math.max(live.anchor, live.focus) : -1;

  // Indices change with the words or the pause length; a corrected word keeps them.
  const shown = useRef(tokens);
  useEffect(() => {
    if (!sameShape(shown.current, tokens)) setSel(null);
    shown.current = tokens;
    setEditing(null);
  }, [tokens]);

  // An AI run that starts meanwhile closes the field without saving.
  useEffect(() => {
    if (lock) setEditing(null);
  }, [lock]);

  // Toasts move above the Delete bar while it shows, so an Undo toast never covers Delete.
  const barShown = lo >= 0;
  useLayoutEffect(() => {
    if (!barShown) return;
    useEditor.setState({ toastLift: bar.current?.offsetHeight ?? 0 });
    return () => useEditor.setState({ toastLift: 0 });
  }, [barShown]);

  useEffect(() => {
    if (playing && active >= 0) box.current?.querySelector(`[data-t="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active, playing]);

  const indexAt = (el: EventTarget | null) => {
    const t = el instanceof Element ? el.closest("[data-t]") : null;
    return t ? Number(t.getAttribute("data-t")) : null;
  };
  const seekTo = (i: number) => useEditor.getState().seek(tokens[i].startUs);

  /** Opens the word for correcting, unless the AI is editing: then the notice says so. */
  const startEdit = (i: number) => {
    if (tokens[i]?.kind !== "word") return;
    if (lock) {
      aiLocked();
      return;
    }
    setSel({ anchor: i, focus: i });
    setEditing(i);
  };

  // Stable, so paragraphs without the field do not re-render with each word played.
  const finish = useRef<(text: string | null) => void>(() => {});
  finish.current = (text) => {
    const t = editing === null ? null : tokens[editing];
    setEditing(null);
    box.current?.focus();
    const typed = text?.trim();
    if (t?.kind === "word" && typed && typed !== t.text) void onCorrect(t.i, typed, t.text);
  };
  const onEdited = useCallback((text: string | null) => finish.current(text), []);

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || e.target instanceof HTMLInputElement) return;
    box.current?.focus();
    const i = indexAt(e.target);
    if (i === null) return;
    e.preventDefault();
    dragging.current = true;
    const stop = () => (dragging.current = false);
    followPointer({ up: stop, cancel: stop });
    if (e.shiftKey && live) setSel({ anchor: live.anchor, focus: i });
    else {
      setSel({ anchor: i, focus: i });
      seekTo(i);
    }
  };

  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (!dragging.current || !(e.buttons & 1) || !live) return;
    const i = indexAt(e.target);
    if (i !== null && i !== live.focus) setSel({ anchor: live.anchor, focus: i });
  };

  const remove = async () => {
    if (lo >= 0 && (await onDelete(lo, hi))) setSel(null);
  };

  /** Delete cuts the words and never falls through to deleting the selected timeline clips. */
  const onDeleteKey = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key !== "Delete" && e.key !== "Backspace") return false;
    e.preventDefault();
    e.stopPropagation();
    if (blocker) useEditor.getState().toast({ kind: "info", text: `${blocker}.` });
    else remove();
    return true;
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (onDeleteKey(e)) return;
    const key = e.key;
    if (tokens.length === 0) return;
    if (key === "ArrowRight" || key === "ArrowLeft" || key === "Home" || key === "End") {
      // Word by word here; the playhead's frame keys apply elsewhere.
      e.preventDefault();
      e.stopPropagation();
      const n = tokens.length;
      const from = live?.focus ?? (active >= 0 ? active : key === "ArrowRight" ? -1 : n);
      const to = key === "Home" ? 0 : key === "End" ? n - 1 : Math.max(0, Math.min(n - 1, from + (key === "ArrowRight" ? 1 : -1)));
      setSel(e.shiftKey && live ? { anchor: live.anchor, focus: to } : { anchor: to, focus: to });
      if (!e.shiftKey) seekTo(to);
      box.current?.querySelector(`[data-t="${to}"]`)?.scrollIntoView({ block: "nearest" });
    } else if (key === " ") {
      // Plays without dropping focus, so Delete keeps acting on the words.
      e.preventDefault();
      e.stopPropagation();
      useEditor.getState().togglePlay();
    } else if (key === "F2" && live && lo === hi) {
      e.preventDefault();
      e.stopPropagation();
      startEdit(lo);
    } else if (key === "Escape" && live) {
      e.stopPropagation();
      setSel(null);
    } else if (key === "Enter" && live) {
      e.preventDefault();
      seekTo(live.focus);
    }
  };

  const picked = lo >= 0 ? tokens.slice(lo, hi + 1) : [];
  const words = picked.filter((t) => t.kind === "word").length;
  const lengthUs = lo >= 0 ? tokens[hi].endUs - tokens[lo].startUs : 0;
  const summary = words > 0 ? `${words} word${words === 1 ? "" : "s"}` : `${picked.length} pause${picked.length === 1 ? "" : "s"}`;

  return (
    <>
      <div
        ref={box}
        role="listbox"
        aria-label="Transcript"
        aria-multiselectable
        aria-activedescendant={live && editing === null ? optionId(live.focus) : undefined}
        tabIndex={0}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onDoubleClick={(e) => {
          const i = indexAt(e.target);
          if (i !== null) startEdit(i);
        }}
        onKeyDown={onKeyDown}
        className="min-h-0 flex-1 overflow-y-auto px-3 pb-1 pt-3 focus-visible:-outline-offset-2"
      >
        {paras.map(([from, to]) => (
          <Paragraph
            key={from}
            tokens={tokens}
            from={from}
            to={to}
            active={active >= from && active < to ? active : -1}
            lo={hi >= from && lo < to ? lo : -1}
            hi={hi >= from && lo < to ? hi : -1}
            editing={editing !== null && editing >= from && editing < to ? editing : -1}
            onEdited={onEdited}
          />
        ))}
      </div>
      {lo >= 0 && (
        <div ref={bar} className="flex shrink-0 items-center gap-2 border-t border-line px-3 py-2" onKeyDown={onDeleteKey}>
          <span className="tabular flex-1 text-[12px] text-muted">
            {summary} · {formatDuration(lengthUs)}
          </span>
          <Button variant="danger" className="h-7" disabled={!!blocker} disabledReason={blocker ?? undefined} title="Cut from the timeline (Delete)" onClick={remove}>
            <Trash2 size={14} /> Delete
          </Button>
        </div>
      )}
    </>
  );
}
