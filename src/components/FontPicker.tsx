import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { AlertTriangle, Check, ChevronDown } from "lucide-react";
import { DEFAULT_FONT, fontCss, useFontFamilies } from "../lib/fonts";

const MENU_W = 260;
const MENU_H = 340;

/**
 * Font chooser: a button showing the current family in its own face, opening a searchable list.
 * Fonts that ship with CapOpen come first and preview in their face; installed ones follow.
 * `mixed` shows "—" for a selection whose clips use different fonts.
 */
export function FontPicker({
  value,
  mixed = false,
  onChange,
  disabled,
  disabledReason,
}: {
  value: string | null | undefined;
  mixed?: boolean;
  onChange: (family: string) => void;
  disabled?: boolean;
  disabledReason?: string;
}) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const { fonts } = useFontFamilies();
  const current = value || DEFAULT_FONT;
  const missing = !mixed && !!fonts && !fonts.bundled.includes(current) && !fonts.system.includes(current);
  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  return (
    <>
      <button
        ref={trigger}
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={`Font: ${mixed ? "mixed" : current}`}
        title={disabled ? disabledReason : missing ? `${current} is not installed on this computer, so ${DEFAULT_FONT} is used` : undefined}
        disabled={disabled}
        onClick={() => setOpen(!open)}
        className="flex h-8 min-w-0 flex-1 items-center gap-1.5 rounded-md border border-line bg-raised pl-2 pr-1.5 text-left text-fg transition-colors duration-[120ms] ease-out enabled:hover:border-muted disabled:cursor-not-allowed disabled:opacity-40"
      >
        <span className="min-w-0 flex-1 truncate text-[15px] leading-5" style={{ fontFamily: mixed ? undefined : fontCss(current) }}>
          {mixed ? "—" : current}
        </span>
        {missing && <AlertTriangle size={14} className="shrink-0 text-warn" aria-label="Not installed" />}
        <ChevronDown size={14} className="shrink-0 text-muted" />
      </button>
      {open && trigger.current && (
        <FontMenu
          anchor={trigger.current}
          value={mixed ? null : current}
          onPick={(f) => {
            close();
            if (mixed || f !== current) onChange(f);
          }}
          onClose={close}
        />
      )}
    </>
  );
}

type Item = { family: string; bundled: boolean };

function FontMenu({ anchor, value, onPick, onClose }: { anchor: HTMLElement; value: string | null; onPick: (family: string) => void; onClose: () => void }) {
  const { fonts, error } = useFontFamilies();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [pos, setPos] = useState<CSSProperties>({ visibility: "hidden" });
  const ref = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const id = useId();

  const items = useMemo<Item[]>(() => {
    if (!fonts) return [];
    const q = query.trim().toLowerCase();
    const match = (f: string) => f.toLowerCase().includes(q);
    return [...fonts.bundled.filter(match).map((family) => ({ family, bundled: true })), ...fonts.system.filter(match).map((family) => ({ family, bundled: false }))];
  }, [fonts, query]);
  const firstSystem = items.findIndex((i) => !i.bundled);

  // Start on the current font; typing jumps to the first match.
  useEffect(() => {
    const i = query ? 0 : items.findIndex((it) => it.family === value);
    setActive(Math.max(0, i));
  }, [items, query, value]);

  useEffect(() => {
    list.current?.querySelector(`[data-i="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active, items]);

  // Below the button when there is room, otherwise above; always inside the window.
  useLayoutEffect(() => {
    const r = anchor.getBoundingClientRect();
    const width = Math.max(r.width, MENU_W);
    const left = Math.max(8, Math.min(r.left, window.innerWidth - width - 8));
    const below = window.innerHeight - r.bottom - 12;
    const above = r.top - 12;
    if (below >= 240 || below >= above) setPos({ left, width, top: r.bottom + 4, maxHeight: Math.min(MENU_H, below) });
    else setPos({ left, width, bottom: window.innerHeight - r.top + 4, maxHeight: Math.min(MENU_H, above) });
  }, [anchor]);

  // After positioning: a hidden field cannot take focus.
  useEffect(() => {
    if (pos.visibility !== "hidden") search.current?.focus();
  }, [pos]);

  useEffect(() => {
    const outside = (e: Event) => !ref.current?.contains(e.target as Node) && !anchor.contains(e.target as Node) && onClose();
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("resize", onClose);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("resize", onClose);
      window.removeEventListener("blur", onClose);
    };
  }, [anchor, onClose]);

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (items.length > 0) setActive((a) => (a + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (items[active]) onPick(items[active].family);
    } else if (e.key === "Escape") {
      // Esc closes the list only; it must not also clear the clip selection.
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === "Tab") onClose();
  };

  const optionId = (i: number) => `${id}-${i}`;

  return (
    <div ref={ref} className="fixed z-[110] flex flex-col overflow-hidden rounded-lg border border-line bg-panel shadow-2xl shadow-black/60" style={pos}>
      <div className="shrink-0 border-b border-line p-1.5">
        <input
          ref={search}
          type="text"
          role="combobox"
          aria-expanded
          aria-controls={`${id}-list`}
          aria-activedescendant={items[active] ? optionId(active) : undefined}
          aria-label="Search fonts"
          placeholder="Search fonts"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          className="h-8 w-full rounded-md border border-line bg-raised px-2 text-[13px] text-fg placeholder:text-muted focus:border-accent"
        />
      </div>
      <div ref={list} id={`${id}-list`} role="listbox" aria-label="Fonts" className="min-h-0 flex-1 overflow-y-auto p-1">
        {!fonts && !error && <p className="px-2 py-2 text-[12px] text-muted">Loading fonts…</p>}
        {error && (
          <p className="flex gap-1.5 px-2 py-2 text-[12px] text-danger" role="alert">
            <AlertTriangle size={14} className="mt-px shrink-0" /> Could not list fonts: {error}
          </p>
        )}
        {fonts && items.length === 0 && <p className="px-2 py-2 text-[12px] text-muted">No font matches “{query.trim()}”</p>}
        {items.map((it, i) => (
          <div key={`${it.bundled}:${it.family}`}>
            {(i === 0 || i === firstSystem) && (
              <div className="px-2 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wide text-muted" role="presentation">
                {it.bundled ? "Built in" : "On this computer"}
              </div>
            )}
            <div
              id={optionId(i)}
              data-i={i}
              role="option"
              aria-selected={it.family === value}
              onPointerDown={(e) => e.preventDefault()}
              onPointerMove={() => active !== i && setActive(i)}
              onClick={() => onPick(it.family)}
              className={`flex h-8 cursor-pointer items-center gap-2 rounded px-2 ${i === active ? "bg-raised" : ""}`}
            >
              <span className={`min-w-0 flex-1 truncate ${it.bundled ? "text-[15px]" : "text-[13px]"} ${it.family === value ? "text-fg" : "text-fg/90"}`} style={it.bundled ? { fontFamily: fontCss(it.family) } : undefined}>
                {it.family}
              </span>
              {it.family === value && <Check size={14} className="shrink-0 text-accent" />}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
