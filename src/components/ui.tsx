import { createContext, useContext, useEffect, useId, useLayoutEffect, useRef, useState, type ButtonHTMLAttributes, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronRight, Minus } from "lucide-react";
import { followPointer } from "../lib/drag";
import { fontCss } from "../lib/fonts";
import { AI_EDITING, useAiLocked } from "../lib/store";
import type { TextStyle } from "../lib/types";

type Variant = "primary" | "secondary" | "ghost" | "danger" | "bar";

const variants: Record<Variant, string> = {
  primary: "btn-prominent font-semibold",
  secondary: "bg-white/[.09] text-fg shadow-[inset_0_1px_0_rgb(255_255_255/.07),inset_0_0_0_1px_rgb(255_255_255/.07)] hover:bg-white/[.14]",
  ghost: "text-muted hover:bg-white/[.08] hover:text-fg",
  danger: "bg-danger/15 text-danger shadow-[inset_0_0_0_1px_rgb(255_69_58/.4)] hover:bg-danger/25",
  /** A floating toolbar capsule, e.g. in the top bar. */
  bar: "bar text-fg hover:bg-white/[.13]",
};

/** Drops hover styles, so a disabled control does not react to the pointer. */
const idle = (classes: string) => classes.split(" ").filter((c) => !c.startsWith("hover:")).join(" ");

/** Why the controls inside an `AiLock` are locked, or null. */
const LockReason = createContext<string | null>(null);
export const useLockReason = () => useContext(LockReason);

/** For a plain button inside an `AiLock`: focusable, inert to clicks, the reason as its tooltip. */
export const lockedProps = (reason: string | null) => (reason ? { "aria-disabled": true as const, title: reason, onClick: undefined } : {});

/**
 * A disabled button here stays focusable (`aria-disabled` rather than `disabled`), so the keyboard
 * and screen readers reach it and its reason: the tooltip, the description, and the hint shown on
 * keyboard focus (`DisabledHint`). A click on it does nothing.
 */
function disabledProps(disabled: boolean | undefined, onClick: ButtonHTMLAttributes<HTMLButtonElement>["onClick"], reasonId?: string) {
  return disabled ? { "aria-disabled": true, onClick: undefined, "aria-describedby": reasonId } : { onClick };
}

export function Button({
  variant = "secondary",
  pill = false,
  className = "",
  children,
  disabled,
  disabledReason,
  title,
  onClick,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; disabledReason?: string; /** Capsule shape, for toolbars. */ pill?: boolean }) {
  const reasonId = useId();
  const lock = useLockReason();
  disabled = disabled || !!lock;
  const reason = lock ?? (disabled ? (disabledReason ?? title) : undefined);
  return (
    <>
      <button
        type="button"
        title={disabled ? reason : title}
        {...disabledProps(disabled, onClick, reason ? reasonId : undefined)}
        className={`btn ${pill ? "btn-pill" : ""} transition-colors duration-[120ms] ease-out disabled:cursor-not-allowed disabled:opacity-40 ${
          disabled ? `cursor-not-allowed opacity-40 ${idle(variants[variant])}` : `active:brightness-90 ${variants[variant]}`
        } ${className}`}
        {...rest}
      >
        {children}
      </button>
      {reason && (
        <span id={reasonId} hidden>
          {reason}
        </span>
      )}
    </>
  );
}

/**
 * Icon-only button. `label` is required: it becomes the tooltip and the accessible name, and says
 * why when the button is disabled ("Split: move the playhead over a clip").
 * Passing `active`, true or false, makes it a toggle button.
 */
export function IconButton({
  label,
  active,
  round = false,
  className = "",
  children,
  disabled,
  onClick,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean; /** Round, for icons grouped in a toolbar capsule. */ round?: boolean }) {
  const lock = useLockReason();
  disabled = disabled || !!lock;
  return (
    <button
      type="button"
      aria-label={label}
      title={lock ?? label}
      aria-pressed={active}
      {...disabledProps(disabled, onClick)}
      className={`icon-btn ${round ? "icon-btn-round" : ""} transition-colors duration-[120ms] ease-out disabled:cursor-not-allowed disabled:opacity-40 ${
        disabled ? "cursor-not-allowed opacity-40" : "active:brightness-75"
      } ${active ? "bg-accent/[.18] text-accent" : disabled ? "text-muted" : "text-fg/80 hover:bg-white/[.08] hover:text-fg"} ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
}

/**
 * While a disabled control has keyboard focus, its reason (the tooltip text) shows just below it,
 * as hovering shows the tooltip. Mount once.
 */
export function DisabledHint() {
  const [hint, setHint] = useState<{ text: string; x: number; y: number; above: boolean } | null>(null);
  const keyboard = useRef(false);
  const show = (el: HTMLElement | null) => {
    const text = el?.getAttribute("aria-disabled") === "true" ? el.getAttribute("title") : null;
    if (!el || !text || !keyboard.current) return setHint(null);
    const r = el.getBoundingClientRect();
    const above = r.bottom + 40 > window.innerHeight;
    setHint({ text, x: Math.max(138, Math.min(window.innerWidth - 138, r.left + r.width / 2)), y: above ? r.top - 6 : r.bottom + 6, above });
  };
  // A run that starts or ends locks or unlocks the focused control, so its hint follows.
  const locked = useAiLocked();
  useEffect(() => {
    if (document.activeElement instanceof HTMLElement) show(document.activeElement);
  }, [locked]);
  useEffect(() => {
    const onKey = () => (keyboard.current = true);
    const onPointer = () => (keyboard.current = false);
    const onFocus = (e: FocusEvent) => show(e.target instanceof HTMLElement ? e.target : null);
    const hide = () => setHint(null);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onPointer, true);
    window.addEventListener("focusin", onFocus);
    window.addEventListener("focusout", hide);
    window.addEventListener("scroll", hide, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pointerdown", onPointer, true);
      window.removeEventListener("focusin", onFocus);
      window.removeEventListener("focusout", hide);
      window.removeEventListener("scroll", hide, true);
    };
  }, []);
  if (!hint) return null;
  return (
    <div
      aria-hidden
      className={`overlay pointer-events-none fixed z-[130] max-w-[260px] -translate-x-1/2 rounded-lg px-2.5 py-1.5 text-[12px] text-fg ${hint.above ? "-translate-y-full" : ""}`}
      style={{ left: hint.x, top: hint.y }}
    >
      {hint.text}
    </div>
  );
}

/**
 * Locks the editing controls inside while an agent's run is open. Buttons stay focusable and give
 * the reason; fields are disabled with the reason as their tooltip.
 */
export function AiLock({ children }: { children: ReactNode }) {
  const locked = useAiLocked();
  return <LockReason.Provider value={locked ? AI_EDITING : null}>{children}</LockReason.Provider>;
}

export function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-[12px] text-muted">{label}</span>
      {children}
      {hint && <span className="text-[11px] text-muted">{hint}</span>}
    </label>
  );
}

/** Accepts "12,5", "50 %" and "-3" alike. */
const parseNumber = (s: string) => Number(s.replace(",", ".").replace(/[^\d.+-]/g, "") || "x");

/**
 * Numeric field that keeps what you type until Enter or blur, so partial input like "-"
 * or "1." is never rejected mid-typing. Esc restores the value; ↑/↓ step (Shift ×10).
 * `mixed` shows "—" for a selection with different values; any typed value then applies to all.
 */
export function NumberInput({
  label,
  value,
  min,
  max,
  step,
  onChange,
  format,
  parse = parseNumber,
  disabled,
  mixed = false,
  className = "w-14",
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
  format: (v: number) => string;
  parse?: (s: string) => number;
  disabled?: boolean;
  mixed?: boolean;
  className?: string;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const cancel = useRef(false);
  const lock = useLockReason();
  disabled = disabled || !!lock;
  const clamp = (v: number) => Math.max(min, Math.min(max, v));
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") e.currentTarget.blur();
    else if (e.key === "Escape") {
      e.stopPropagation();
      cancel.current = true;
      e.currentTarget.blur();
    } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      e.preventDefault();
      const base = draft !== null && Number.isFinite(parse(draft)) ? parse(draft) : value;
      const next = clamp(base + (e.key === "ArrowUp" ? 1 : -1) * step * (e.shiftKey ? 10 : 1));
      onChange(next);
      setDraft(format(next));
    }
  };
  return (
    <input
      aria-label={label}
      inputMode="decimal"
      disabled={disabled}
      className={`tabular h-6 rounded-md border px-1.5 text-right text-[12px] outline-offset-0 focus:border-accent ${
        lock ? "border-transparent bg-transparent text-fg/80" : "border-white/[.08] bg-white/[.055] text-fg disabled:opacity-40"
      } ${className}`}
      value={draft ?? (mixed ? "—" : format(value))}
      onFocus={(e) => {
        setDraft(mixed ? "" : format(value));
        e.currentTarget.select();
      }}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => {
        const v = draft === null || draft.trim() === "" ? NaN : parse(draft);
        if (!cancel.current && Number.isFinite(v) && (mixed || format(clamp(v)) !== format(value))) onChange(clamp(v));
        cancel.current = false;
        setDraft(null);
      }}
      onKeyDown={onKeyDown}
    />
  );
}

const THUMB = 22;

/**
 * Range input with a filled track. One-sided ranges fill from the left; two-sided ones
 * (min < 0 < max, e.g. temperature or rotation) fill from 0 to the thumb and mark 0 with a tick.
 */
export function RangeInput({
  label,
  value,
  min,
  max,
  step,
  onChange,
  disabled,
  valueText,
  className = "",
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
  disabled?: boolean;
  /** What the value means to a person, for sliders whose position is not the value, such as logarithmic ones. */
  valueText?: string;
  className?: string;
}) {
  const lock = useLockReason();
  disabled = disabled || !!lock;
  const frac = (v: number) => (max > min ? Math.max(0, Math.min(1, (v - min) / (max - min))) : 0);
  const twoSided = min < 0 && max > 0;
  const origin = twoSided ? frac(0) : 0;
  const at = (f: number) => `calc(${THUMB / 2}px + ${f} * (100% - ${THUMB}px))`;
  const [lo, hi] = [Math.min(origin, frac(value)), Math.max(origin, frac(value))];
  return (
    <span className={`relative flex h-4 items-center ${className}`}>
      {twoSided && <span aria-hidden className={`pointer-events-none absolute top-1 h-2 w-0.5 -translate-x-1/2 rounded-full ${disabled ? "bg-white/15" : "bg-white/30"}`} style={{ left: at(origin) }} />}
      <input
        type="range"
        aria-label={label}
        min={min}
        max={max}
        step={step}
        value={value}
        aria-valuetext={valueText}
        disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value))}
        className={`range relative w-full ${lock ? "range-locked" : ""}`}
        style={{ "--fill-from": twoSided ? at(lo) : "0px", "--fill-to": at(hi) } as CSSProperties}
      />
    </span>
  );
}

/** The label above, then the slider and a typeable value on one row, like CapCut's property rows. */
export function Slider({
  label,
  value,
  min,
  max,
  step,
  unit = "",
  onChange,
  format = (v) => String(Math.round(v * 100) / 100),
  parse,
  disabled,
  mixed,
  title,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  unit?: string;
  onChange: (v: number) => void;
  format?: (v: number) => string;
  parse?: (s: string) => number;
  disabled?: boolean;
  /** The selected clips differ; the thumb sits at the first clip's value. */
  mixed?: boolean;
  title?: string;
}) {
  const lock = useLockReason();
  disabled = disabled || !!lock;
  return (
    // 8 px between two sliders instead of the section's 12 px, so a column of them reads as one list.
    <div className="flex flex-col gap-0.5 [&+&]:-mt-1" title={lock ?? title}>
      <span className={`truncate text-[12px] leading-4 ${disabled && !lock ? "text-subtle" : "text-muted"}`}>{label}</span>
      <div className="flex items-center gap-2">
        <RangeInput
          label={label}
          value={value}
          min={min}
          max={max}
          step={step}
          onChange={onChange}
          disabled={disabled}
          valueText={mixed ? "Mixed" : `${format(value)}${unit && ` ${unit}`}`}
          className="min-w-0 flex-1"
        />
        <span className="flex shrink-0 items-center gap-0.5">
          <NumberInput label={`${label} value`} value={value} min={min} max={max} step={step} onChange={onChange} format={format} parse={parse} disabled={disabled} mixed={mixed} />
          <span className="w-3 text-[11px] text-muted">{unit}</span>
        </span>
      </div>
    </div>
  );
}

export function Section({ title, children, actions }: { title: string; children: ReactNode; actions?: ReactNode }) {
  return (
    <section className="flex flex-col gap-3 border-b border-white/[.07] px-4 py-4 last:border-b-0">
      <div className="flex min-h-6 items-center justify-between gap-2">
        <h3 className="text-[13px] font-semibold text-fg">{title}</h3>
        {actions && <div className="flex items-center gap-0.5">{actions}</div>}
      </div>
      {children}
    </section>
  );
}

/** `mixed`: several clips with different colours; it shows a dash until one is picked for all of them. */
export function ColorInput({ label, value, onChange, mixed = false }: { label: string; value: string; onChange: (v: string) => void; mixed?: boolean }) {
  const lock = useLockReason();
  return (
    <label className="flex items-center justify-between gap-2 has-[:disabled]:opacity-40" title={lock ?? undefined}>
      <span className="text-[12px] text-muted">{label}</span>
      <span className="flex items-center gap-2">
        <span className="tabular text-[12px] text-muted">{mixed ? "—" : value.slice(0, 7).toUpperCase()}</span>
        {/* WebKitGTK draws the colour well like a switch, so a plain swatch sits over a see-through input. */}
        <span
          className="relative h-5 w-8 rounded-[5px] shadow-[inset_0_0_0_1px_rgb(255_255_255/.18)] has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent"
          style={{ background: mixed ? "transparent" : value.slice(0, 7) }}
        >
          <input
            type="color"
            aria-label={label}
            disabled={!!lock}
            value={value.slice(0, 7)}
            onChange={(e) => onChange(e.target.value + value.slice(7))}
            className="absolute inset-0 h-full w-full cursor-pointer opacity-0 disabled:cursor-not-allowed"
          />
        </span>
      </span>
    </label>
  );
}

export function Checkbox({
  label,
  checked,
  onChange,
  disabled,
  title,
  mixed = false,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  title?: string;
  /** Some of the selected clips have it on: a dash, and checking turns it on for all of them. */
  mixed?: boolean;
}) {
  const lock = useLockReason();
  disabled = disabled || !!lock;
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (input.current) input.current.indeterminate = mixed;
  }, [mixed]);
  return (
    <label className="flex cursor-pointer items-center gap-2 has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-40" title={lock ?? title}>
      <span className="relative inline-flex">
        <input
          ref={input}
          type="checkbox"
          checked={checked && !mixed}
          disabled={disabled}
          onChange={(e) => onChange(e.target.checked)}
          className="peer absolute inset-0 m-0 h-full w-full cursor-[inherit] opacity-0"
        />
        <span
          aria-hidden
          className={`flex h-4 w-4 items-center justify-center rounded-[4px] border transition-colors duration-[120ms] ease-out peer-focus-visible:outline-2 peer-focus-visible:outline-offset-1 peer-focus-visible:outline-accent ${
            checked || mixed ? "border-accent-strong bg-accent-strong text-white" : "border-white/25 bg-white/[.07]"
          }`}
        >
          {mixed ? <Minus size={12} strokeWidth={3} /> : checked && <Check size={12} strokeWidth={3} />}
        </span>
      </span>
      <span className="text-[12px] text-fg">{label}</span>
    </label>
  );
}

/**
 * Keyboard for a tablist: ←/→ move to the previous/next tab (wrapping), Home/End to the first/last,
 * and focus follows. The keys stop here, so they never also move the playhead.
 * Tabs carry `data-tab={id}`.
 */
export function tabListKeys<T extends string>(ids: readonly T[], value: T, onChange: (id: T) => void) {
  return (e: KeyboardEvent<HTMLElement>) => {
    const i = ids.indexOf(value);
    const n = ids.length;
    const to = e.key === "ArrowRight" ? (i + 1) % n : e.key === "ArrowLeft" ? (i + n - 1) % n : e.key === "Home" ? 0 : e.key === "End" ? n - 1 : -1;
    if (to < 0) return;
    e.preventDefault();
    e.stopPropagation();
    onChange(ids[to]);
    e.currentTarget.querySelector<HTMLElement>(`[data-tab="${ids[to]}"]`)?.focus();
  };
}

/** Ids that tie a tab to its panel, unique per tab `group`. */
export const tabIds = (group: string, id: string) => ({ tab: `${group}-tab-${id}`, panel: `${group}-panel-${id}` });

/** The content of the selected tab, named by its tab. */
export function TabPanel({ group, id, className, children }: { group: string; id: string; className?: string; children: ReactNode }) {
  const ids = tabIds(group, id);
  return (
    <div role="tabpanel" id={ids.panel} aria-labelledby={ids.tab} className={className}>
      {children}
    </div>
  );
}

/** Text tabs with an underline, arrow keys move between them. The selected tab's content goes in a `TabPanel` of the same `group`. */
export function TabBar<T extends string>({
  group,
  tabs,
  value,
  onChange,
  label,
  className = "mx-3 mb-1",
}: {
  group: string;
  tabs: { id: T; label: string }[];
  value: T;
  onChange: (id: T) => void;
  label: string;
  /** Its margins; the inspector's by default. */
  className?: string;
}) {
  const onKeyDown = tabListKeys(
    tabs.map((t) => t.id),
    value,
    onChange,
  );
  return (
    <div role="tablist" aria-label={label} className={`seg-track flex shrink-0 gap-0.5 overflow-hidden rounded-[9px] p-0.5 ${className}`} onKeyDown={onKeyDown}>
      {tabs.map((t) => (
        <button
          key={t.id}
          type="button"
          role="tab"
          id={tabIds(group, t.id).tab}
          data-tab={t.id}
          aria-selected={t.id === value}
          aria-controls={t.id === value ? tabIds(group, t.id).panel : undefined}
          tabIndex={t.id === value ? 0 : -1}
          onClick={() => onChange(t.id)}
          className={`h-7 min-w-0 flex-auto truncate rounded-[7px] px-1 text-[12px] font-medium whitespace-nowrap transition-colors duration-[120ms] ease-out ${
            t.id === value ? "seg-on text-fg" : "text-muted hover:text-fg"
          }`}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}

/** Small exclusive choice, e.g. animation In / Out or speed presets. */
export function Segmented<T extends string | number>({
  options,
  value,
  onChange,
  label,
  disabled = false,
  disabledReason,
}: {
  options: { id: T; label: ReactNode; title?: string }[];
  value: T | null;
  onChange: (id: T) => void;
  label: string;
  disabled?: boolean;
  disabledReason?: string;
}) {
  const reasonId = useId();
  const lock = useLockReason();
  disabled = disabled || !!lock;
  const reason = lock ?? (disabled ? disabledReason : undefined);
  return (
    <div role="group" aria-label={label} className="seg-track flex gap-0.5 rounded-[9px] p-0.5">
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          aria-pressed={o.id === value}
          title={disabled ? reason : o.title}
          {...disabledProps(disabled, () => onChange(o.id), reason ? reasonId : undefined)}
          className={`tabular h-7 flex-1 rounded-[7px] px-2 text-[12px] font-medium whitespace-nowrap transition-colors duration-[120ms] ease-out disabled:cursor-not-allowed disabled:opacity-40 aria-disabled:cursor-not-allowed aria-disabled:opacity-40 ${
            o.id === value ? "seg-on text-fg" : `text-muted ${disabled ? "" : "enabled:hover:text-fg"}`
          }`}
        >
          {o.label}
        </button>
      ))}
      {reason && (
        <span id={reasonId} hidden>
          {reason}
        </span>
      )}
    </div>
  );
}

/** Preset grid item: preview on top, label below; the selected one carries a check. */
export function PresetTile({
  label,
  selected,
  onClick,
  disabled,
  title,
  children,
}: {
  label: string;
  selected: boolean;
  onClick: () => void;
  disabled?: boolean;
  title?: string;
  children: ReactNode;
}) {
  const lock = useLockReason();
  disabled = disabled || !!lock;
  return (
    <button
      type="button"
      aria-pressed={selected}
      title={lock ?? title ?? label}
      {...disabledProps(disabled, onClick)}
      className="group flex min-w-0 flex-col gap-1.5 rounded-[10px] text-left disabled:cursor-not-allowed disabled:opacity-40 aria-disabled:cursor-not-allowed aria-disabled:opacity-40"
    >
      <span
        className={`relative block aspect-[4/3] w-full overflow-hidden rounded-[10px] border bg-bg ${
          selected ? "border-accent shadow-[0_0_0_1px_var(--color-accent)]" : disabled ? "border-white/[.08]" : "border-white/[.08] group-enabled:group-hover:border-white/30"
        }`}
      >
        {children}
        {selected && (
          <span className="absolute right-1 top-1 flex h-4 w-4 items-center justify-center rounded-full bg-accent-strong text-white shadow-[0_1px_2px_rgb(0_0_0/.5)]">
            <Check size={11} strokeWidth={3} />
          </span>
        )}
      </span>
      <span className={`truncate text-[11px] ${selected ? "font-medium text-fg" : "text-muted"}`}>{label}</span>
    </button>
  );
}

const SPLITTER_KEYS: Record<string, number> = { ArrowLeft: -24, ArrowRight: 24, ArrowUp: -24, ArrowDown: 24 };

/**
 * The 6 px gap between two panes, which resizes the pane on one side: drag it, double-click to
 * reset, or ←/→ (↑/↓ across a horizontal gap) by 24 px while it has focus. A short grip shows on
 * hover, and in `accent` while dragging or focused from the keyboard, in place of a focus ring
 * that would run along the edges of the panes. `grow` is 1 when moving the gap right or down enlarges the pane, -1
 * when moving it left or up does. `className` places it: `relative` with a negative margin in a
 * row or column of panes, `absolute` over a gap it does not own.
 */
export function Splitter({
  what,
  axis,
  size,
  min,
  max,
  grow,
  onResize,
  onReset,
  className,
}: {
  /** The pane it resizes, as in "Resize the timeline". */
  what: string;
  /** "x" for a gap between panes side by side. */
  axis: "x" | "y";
  size: number;
  min: number;
  max: number;
  grow: 1 | -1;
  onResize: (size: number) => void;
  onReset: () => void;
  className: string;
}) {
  const [active, setActive] = useState(false);
  const x = axis === "x";
  const set = (v: number) => onResize(Math.round(Math.max(min, Math.min(max, v))));
  return (
    <div
      role="separator"
      aria-orientation={x ? "vertical" : "horizontal"}
      aria-label={`Resize ${what}`}
      aria-valuenow={size}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      title={`Drag to resize the ${what}. Double-click to reset.`}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        const start = x ? e.clientX : e.clientY;
        setActive(true);
        // A cancelled drag keeps the size reached so far.
        followPointer({ move: (ev) => set(size + grow * ((x ? ev.clientX : ev.clientY) - start)), up: () => setActive(false), cancel: () => setActive(false) });
      }}
      onDoubleClick={onReset}
      onKeyDown={(e) => {
        const step = SPLITTER_KEYS[e.key];
        if (!step || x !== (e.key === "ArrowLeft" || e.key === "ArrowRight")) return;
        e.preventDefault();
        e.stopPropagation();
        set(size + grow * step);
      }}
      className={`group z-30 shrink-0 rounded-full focus-visible:outline-none ${x ? "w-2.5 cursor-col-resize" : "h-2.5 cursor-row-resize"} ${className}`}
    >
      <div
        className={`absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 rounded-full transition-colors duration-[120ms] ${x ? "h-10 w-1" : "h-1 w-10"} ${active ? "bg-accent" : "group-hover:bg-white/30 group-focus-visible:bg-accent"}`}
      />
    </div>
  );
}

/** Determinate when `value` is above 0, otherwise an indeterminate sweep. `label` names the work in progress. */
export function ProgressBar({ value, label, className = "" }: { value: number; label: string; className?: string }) {
  const known = value > 0;
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={known ? Math.round(value * 100) : undefined}
      className={`h-1.5 overflow-hidden rounded-full bg-white/10 ${className}`}
    >
      {known ? (
        <div className="h-full origin-left rounded-full bg-accent transition-transform duration-200" style={{ transform: `scaleX(${Math.max(0.02, value)})` }} />
      ) : (
        <div className="indeterminate h-full w-1/3 rounded-full bg-accent" />
      )}
    </div>
  );
}

/** A style preview in the font the engine draws it with. */
export function TextSwatch({ style, label, size = 15 }: { style: TextStyle; label: string; /** Font size in px. */ size?: number }) {
  // Karaoke styles show the moment a word is spoken: the last word, or the end of a one-word label, lit.
  // Key words take the first word, or the start of a one-word label.
  const split = label.lastIndexOf(" ") + 1 || Math.ceil(label.length / 2);
  const key = style.keywords ? label.indexOf(" ") + 1 || Math.floor(label.length / 2) : 0;
  return (
    <span
      className="inline-block max-w-full truncate rounded px-1 leading-6"
      style={{
        fontSize: size,
        fontFamily: fontCss(style.fontFamily),
        color: style.color,
        fontWeight: style.bold ? 700 : 400,
        background: style.background ?? undefined,
        WebkitTextStroke: style.strokeWidth > 0 ? `${Math.min(2, style.strokeWidth / 4)}px ${style.strokeColor}` : undefined,
        paintOrder: "stroke fill",
      }}
    >
      {key > 0 && <span style={{ color: style.keywords?.color }}>{label.slice(0, Math.min(key, style.highlight ? split : label.length))}</span>}
      {style.highlight ? (
        <>
          {label.slice(key, split)}
          <span style={{ color: style.highlight }}>{label.slice(Math.max(key, split))}</span>
        </>
      ) : (
        label.slice(key)
      )}
    </span>
  );
}

/** Keeps Tab and Shift+Tab inside a modal dialog. */
export function trapTab(e: KeyboardEvent<HTMLElement>) {
  if (e.key !== "Tab") return;
  const items = [...e.currentTarget.querySelectorAll<HTMLElement>("button, input, select, textarea, [tabindex]")].filter(
    (el) => !el.matches(":disabled, [tabindex='-1']") && el.offsetParent !== null,
  );
  if (items.length === 0) return;
  const [first, last] = [items[0], items[items.length - 1]];
  const inside = items.includes(document.activeElement as HTMLElement);
  if (e.shiftKey && (document.activeElement === first || !inside)) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && (document.activeElement === last || !inside)) {
    e.preventDefault();
    first.focus();
  }
}

export type MenuEntry =
  | {
      label: string;
      icon?: ReactNode;
      /** Shown on the right, e.g. "F2". */
      shortcut?: string;
      run?: () => void;
      /** Why it cannot be chosen now; it stays reachable and says so. */
      disabled?: string | null;
      danger?: boolean;
      /** The current choice, marked with a check. */
      checked?: boolean;
      submenu?: MenuEntry[];
    }
  | "separator";

type MenuItemEntry = Exclude<MenuEntry, "separator">;

/**
 * A menu at a point: a context menu, or under the button that opened it (`align: "end"` puts its
 * right edge at `x`). ↑/↓ move, Enter or Space chooses, → opens a submenu and ← closes it, Esc closes,
 * and so do Tab and a click elsewhere. `onClose(chose)` says whether an item ran, so the caller can
 * return focus to what opened it. `keyboard` marks the first item at once, as when opened by a key.
 */
export function Menu({
  items,
  at,
  label,
  onClose,
  keyboard = false,
  minWidth = 220,
}: {
  items: MenuEntry[];
  at: { x: number; y: number; align?: "start" | "end"; /** Opens upwards from `y`, e.g. from a bar at the bottom. */ above?: boolean };
  label: string;
  onClose: (chose: boolean) => void;
  keyboard?: boolean;
  minWidth?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const id = useId();
  const choices = items.flatMap((item, i) => (item === "separator" ? [] : [i]));
  const [active, setActive] = useState(keyboard ? (choices[0] ?? -1) : -1);
  const [open, setOpen] = useState<{ index: number; keyboard: boolean } | null>(null);
  const [place, setPlace] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    const x = at.align === "end" ? at.x - width : at.x;
    const left = Math.max(8, Math.min(x, window.innerWidth - width - 8));
    const top = at.above ? Math.max(8, at.y - height) : at.y + height > window.innerHeight - 8 ? Math.max(8, window.innerHeight - height - 8) : at.y;
    setPlace({ left, top });
    // A hidden element cannot take focus, so it shows in place now rather than with the next render.
    Object.assign(el.style, { left: `${left}px`, top: `${top}px`, visibility: "visible" });
    el.focus({ preventScroll: true });
  }, [at.x, at.y, at.align, at.above]);

  useEffect(() => {
    const outside = (e: PointerEvent) => !(e.target as Element | null)?.closest?.("[data-menu]") && onClose(false);
    const resized = () => onClose(false);
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("resize", resized);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("resize", resized);
    };
  }, [onClose]);

  const choose = (index: number, viaKeyboard: boolean) => {
    const item = items[index] as MenuItemEntry | undefined;
    if (!item || item.disabled) return;
    if (item.submenu) return setOpen({ index, keyboard: viaKeyboard });
    onClose(true);
    item.run?.();
  };
  const step = (dir: 1 | -1) => {
    if (choices.length === 0) return;
    const at = choices.indexOf(active);
    setActive(choices[at < 0 ? (dir === 1 ? 0 : choices.length - 1) : (at + dir + choices.length) % choices.length]);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    // Keys stay in the menu: they never also reach the editor or the grid behind it.
    e.stopPropagation();
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      step(e.key === "ArrowDown" ? 1 : -1);
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      setActive(e.key === "Home" ? choices[0] : choices[choices.length - 1]);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (active >= 0) choose(active, true);
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      if (active >= 0 && (items[active] as MenuItemEntry).submenu) choose(active, true);
    } else if (e.key === "Escape" || e.key === "ArrowLeft") {
      e.preventDefault();
      onClose(false);
    } else if (e.key === "Tab") {
      e.preventDefault();
      onClose(false);
    } else if (e.key.length === 1) {
      // Type-ahead: the next item starting with that letter.
      const from = choices.indexOf(active);
      const next = [...choices.slice(from + 1), ...choices.slice(0, from + 1)].find((i) => (items[i] as MenuItemEntry).label.toLowerCase().startsWith(e.key.toLowerCase()));
      if (next !== undefined) setActive(next);
    }
  };

  const submenu = open && (items[open.index] as MenuItemEntry).submenu;
  const anchor = open && ref.current?.querySelector<HTMLElement>(`[data-index="${open.index}"]`)?.getBoundingClientRect();
  return createPortal(
    <>
      <div
        ref={ref}
        data-menu
        role="menu"
        aria-label={label}
        tabIndex={-1}
        aria-activedescendant={active >= 0 ? `${id}-${active}` : undefined}
        onKeyDown={onKeyDown}
        onContextMenu={(e) => e.preventDefault()}
        className="overlay fixed z-[140] w-max max-w-[min(420px,calc(100vw-16px))] rounded-xl p-1 text-[13px] text-fg outline-none"
        style={{ minWidth, left: place?.left ?? at.x, top: place?.top ?? at.y, visibility: place ? "visible" : "hidden" }}
      >
        {items.map((item, i) =>
          item === "separator" ? (
            <div key={i} role="separator" className="mx-2 my-1 h-px bg-white/[.08]" />
          ) : (
            <div
              key={i}
              id={`${id}-${i}`}
              data-index={i}
              role={item.checked !== undefined ? "menuitemradio" : "menuitem"}
              aria-checked={item.checked}
              aria-disabled={item.disabled ? true : undefined}
              aria-haspopup={item.submenu ? "menu" : undefined}
              aria-expanded={item.submenu ? open?.index === i : undefined}
              title={item.disabled ?? undefined}
              onPointerEnter={() => {
                setActive(i);
                if (item.submenu && !item.disabled) setOpen({ index: i, keyboard: false });
                else if (open && open.index !== i) {
                  // The submenu had the focus; the keys go on working here once it closes.
                  ref.current?.focus({ preventScroll: true });
                  setOpen(null);
                }
              }}
              onClick={() => choose(i, false)}
              className={`flex h-7 items-center gap-2 rounded-md px-2 ${active === i ? "bg-white/[.08]" : ""} ${
                item.disabled ? "opacity-40" : item.danger ? "text-danger" : ""
              }`}
            >
              <span className={`flex w-4 shrink-0 justify-center ${item.danger ? "text-danger" : item.checked ? "text-accent" : "text-muted"}`}>
                {item.checked ? <Check size={15} /> : item.icon}
              </span>
              <span className="min-w-0 flex-1 truncate whitespace-nowrap">{item.label}</span>
              {item.shortcut && <span className="min-w-0 truncate pl-4 text-[12px] text-muted">{item.shortcut}</span>}
              {item.submenu && <ChevronRight size={14} className="shrink-0 text-muted" />}
            </div>
          ),
        )}
      </div>
      {submenu && anchor && (
        <Menu
          items={submenu}
          label={(items[open.index] as MenuItemEntry).label}
          keyboard={open.keyboard}
          minWidth={200}
          at={{ x: anchor.right + 220 > window.innerWidth ? anchor.left - 4 : anchor.right + 4, y: anchor.top - 4, align: anchor.right + 220 > window.innerWidth ? "end" : "start" }}
          onClose={(chose) => {
            setOpen(null);
            if (chose) onClose(true);
            else ref.current?.focus({ preventScroll: true });
          }}
        />
      )}
    </>,
    document.body,
  );
}
