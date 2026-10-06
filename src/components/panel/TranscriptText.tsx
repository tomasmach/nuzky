import { Fragment, memo, useEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { Trash2 } from "lucide-react";
import { formatSeconds, paragraphs, tokenAt, type Token } from "../../lib/speech";
import { useEditor } from "../../lib/store";
import { formatTime } from "../../lib/time";
import { Button } from "../ui";

interface Selection {
  anchor: number;
  focus: number;
}

const optionId = (i: number) => `transcript-token-${i}`;

/** One paragraph; re-renders only when the current word or the selection inside it changes. */
const Paragraph = memo(function Paragraph({ tokens, from, to, active, lo, hi }: { tokens: Token[]; from: number; to: number; active: number; lo: number; hi: number }) {
  const items = [];
  for (let i = from; i < to; i++) {
    const t = tokens[i];
    const selected = i >= lo && i <= hi;
    const tone = i === active ? "text-accent" : selected ? "text-fg" : "";
    const fill = selected ? "bg-accent/30" : "hover:bg-raised";
    items.push(
      <Fragment key={i}>
        {i > from && " "}
        {t.kind === "word" ? (
          <span id={optionId(i)} data-t={i} role="option" aria-selected={selected} className={`cursor-pointer rounded-sm ${fill} ${tone}`}>
            {t.text}
          </span>
        ) : (
          <span
            id={optionId(i)}
            data-t={i}
            role="option"
            aria-selected={selected}
            aria-label={`Pause, ${formatSeconds(t.gapUs)}`}
            title={`Pause of ${formatSeconds(t.gapUs)}`}
            className={`tabular cursor-pointer rounded px-1 py-px text-[11px] ${selected ? "bg-accent/30" : "bg-raised hover:bg-line"} ${tone || "text-muted"}`}
          >
            {formatSeconds(t.gapUs)}
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

/**
 * The transcript as text. Click a word to jump there, drag or Shift-click to select a range,
 * Delete cuts it from the timeline. ←/→ move word by word (Shift extends), Esc clears.
 */
export function TranscriptText({ tokens, disabled, onDelete }: { tokens: Token[]; disabled: boolean; onDelete: (lo: number, hi: number) => Promise<boolean> }) {
  const [sel, setSel] = useState<Selection | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const timeUs = useEditor((s) => s.timeUs);
  const playing = useEditor((s) => s.playing);
  const paras = useMemo(() => paragraphs(tokens), [tokens]);
  const active = disabled ? -1 : tokenAt(tokens, timeUs);
  // Fewer tokens can arrive before the effect below clears the selection.
  const live = sel && sel.anchor < tokens.length && sel.focus < tokens.length ? sel : null;
  const lo = live ? Math.min(live.anchor, live.focus) : -1;
  const hi = live ? Math.max(live.anchor, live.focus) : -1;

  // Indices change with the words or the pause length.
  useEffect(() => setSel(null), [tokens, disabled]);

  useEffect(() => {
    if (playing && active >= 0) box.current?.querySelector(`[data-t="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active, playing]);

  useEffect(() => {
    const up = () => (dragging.current = false);
    window.addEventListener("pointerup", up);
    return () => window.removeEventListener("pointerup", up);
  }, []);

  const indexAt = (el: EventTarget | null) => {
    const t = el instanceof Element ? el.closest("[data-t]") : null;
    return t ? Number(t.getAttribute("data-t")) : null;
  };
  const seekTo = (i: number) => useEditor.getState().seek(tokens[i].startUs);

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || disabled) return;
    box.current?.focus();
    const i = indexAt(e.target);
    if (i === null) return;
    e.preventDefault();
    dragging.current = true;
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
    if (!disabled) remove();
    return true;
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (onDeleteKey(e)) return;
    const key = e.key;
    if (disabled || tokens.length === 0) return;
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
        aria-disabled={disabled || undefined}
        aria-activedescendant={live ? optionId(live.focus) : undefined}
        tabIndex={disabled ? -1 : 0}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onKeyDown={onKeyDown}
        className={`min-h-0 flex-1 overflow-y-auto px-3 pb-1 pt-3 focus-visible:-outline-offset-2 ${disabled ? "opacity-40" : ""}`}
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
          />
        ))}
      </div>
      {lo >= 0 && (
        <div className="flex shrink-0 items-center gap-2 border-t border-line px-3 py-2" onKeyDown={onDeleteKey}>
          <span className="tabular flex-1 text-[12px] text-muted">
            {summary} · {formatSeconds(lengthUs)}
          </span>
          <Button variant="danger" className="h-7" title="Cut from the timeline (Delete)" onClick={remove}>
            <Trash2 size={14} /> Delete
          </Button>
        </div>
      )}
    </>
  );
}
