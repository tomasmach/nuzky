import { useRef, useState, type ButtonHTMLAttributes, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import { Check } from "lucide-react";
import { fontCss } from "../lib/fonts";
import type { TextStyle } from "../lib/types";

type Variant = "primary" | "secondary" | "ghost" | "danger";

const variants: Record<Variant, string> = {
  primary: "bg-accent text-black hover:bg-accent-strong hover:text-white font-semibold",
  secondary: "bg-raised text-fg hover:bg-line border border-line",
  ghost: "text-muted hover:text-fg hover:bg-raised",
  danger: "bg-danger/15 text-danger hover:bg-danger/25 border border-danger/40",
};

export function Button({
  variant = "secondary",
  className = "",
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant }) {
  return (
    <button
      type="button"
      className={`inline-flex h-8 items-center justify-center gap-1.5 rounded-md px-3 text-[13px] transition-colors duration-[120ms] ease-out active:translate-y-px disabled:cursor-not-allowed disabled:opacity-40 disabled:active:translate-y-0 ${variants[variant]} ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
}

/**
 * Icon-only button. `label` is required: it becomes the tooltip and the accessible name.
 * Passing `active`, true or false, makes it a toggle button.
 */
export function IconButton({
  label,
  active,
  className = "",
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={active}
      className={`inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md transition-colors duration-[120ms] ease-out active:translate-y-px disabled:cursor-not-allowed disabled:opacity-35 disabled:active:translate-y-0 ${
        active ? "bg-accent/20 text-accent" : "text-muted hover:bg-raised hover:text-fg"
      } ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
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
      className={`tabular h-6 rounded border border-line bg-raised px-1.5 text-right text-[12px] text-fg focus:border-accent disabled:opacity-40 ${className}`}
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

const THUMB = 14;

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
  className = "",
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
  disabled?: boolean;
  className?: string;
}) {
  const frac = (v: number) => (max > min ? Math.max(0, Math.min(1, (v - min) / (max - min))) : 0);
  const twoSided = min < 0 && max > 0;
  const origin = twoSided ? frac(0) : 0;
  const at = (f: number) => `calc(${THUMB / 2}px + ${f} * (100% - ${THUMB}px))`;
  const [lo, hi] = [Math.min(origin, frac(value)), Math.max(origin, frac(value))];
  return (
    <span className={`relative flex h-4 items-center ${className}`}>
      {twoSided && <span aria-hidden className="pointer-events-none absolute top-0.5 h-3 w-0.5 -translate-x-1/2 rounded-full bg-muted" style={{ left: at(origin) }} />}
      <input
        type="range"
        aria-label={label}
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value))}
        className="range relative w-full disabled:opacity-40"
        style={{ "--fill-from": at(lo), "--fill-to": at(hi) } as CSSProperties}
      />
    </span>
  );
}

/** Label, slider and a typeable value on one row, like CapCut's property rows. */
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
  return (
    <div className="flex items-center gap-2" title={title}>
      <span className={`w-[76px] shrink-0 truncate text-[12px] ${disabled ? "text-subtle" : "text-muted"}`}>{label}</span>
      <RangeInput label={label} value={value} min={min} max={max} step={step} onChange={onChange} disabled={disabled} className="min-w-0 flex-1" />
      <span className="flex shrink-0 items-center gap-0.5">
        <NumberInput label={`${label} value`} value={value} min={min} max={max} step={step} onChange={onChange} format={format} parse={parse} disabled={disabled} mixed={mixed} />
        <span className="w-3 text-[11px] text-muted">{unit}</span>
      </span>
    </div>
  );
}

export function Section({ title, children, actions }: { title: string; children: ReactNode; actions?: ReactNode }) {
  return (
    <section className="flex flex-col gap-3 border-b border-line px-4 py-4 last:border-b-0">
      <div className="flex min-h-6 items-center justify-between gap-2">
        <h3 className="text-[12px] font-semibold uppercase tracking-wide text-muted">{title}</h3>
        {actions && <div className="flex items-center gap-0.5">{actions}</div>}
      </div>
      {children}
    </section>
  );
}

export function ColorInput({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  return (
    <label className="flex items-center justify-between gap-2">
      <span className="text-[12px] text-muted">{label}</span>
      <span className="flex items-center gap-2">
        <span className="tabular text-[12px] text-muted">{value.slice(0, 7).toUpperCase()}</span>
        <input
          type="color"
          aria-label={label}
          value={value.slice(0, 7)}
          onChange={(e) => onChange(e.target.value + value.slice(7))}
          className="h-7 w-9 cursor-pointer rounded border border-line bg-raised p-0.5"
        />
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
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  title?: string;
}) {
  return (
    <label className={`flex items-center gap-2 ${disabled ? "cursor-not-allowed opacity-40" : "cursor-pointer"}`} title={title}>
      <span className="relative inline-flex">
        <input
          type="checkbox"
          checked={checked}
          disabled={disabled}
          onChange={(e) => onChange(e.target.checked)}
          className="peer absolute inset-0 m-0 h-full w-full cursor-[inherit] opacity-0"
        />
        <span
          aria-hidden
          className={`flex h-4 w-4 items-center justify-center rounded border transition-colors duration-[120ms] ease-out peer-focus-visible:outline-2 peer-focus-visible:outline-offset-1 peer-focus-visible:outline-accent ${
            checked ? "border-accent bg-accent text-black" : "border-muted/60 bg-raised"
          }`}
        >
          {checked && <Check size={12} strokeWidth={3} />}
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

/** Text tabs with an underline, arrow keys move between them. */
export function TabBar<T extends string>({ tabs, value, onChange, label }: { tabs: { id: T; label: string }[]; value: T; onChange: (id: T) => void; label: string }) {
  const onKeyDown = tabListKeys(
    tabs.map((t) => t.id),
    value,
    onChange,
  );
  return (
    <div role="tablist" aria-label={label} className="flex shrink-0 gap-1 overflow-x-auto border-b border-line px-2" onKeyDown={onKeyDown}>
      {tabs.map((t) => (
        <button
          key={t.id}
          type="button"
          role="tab"
          data-tab={t.id}
          aria-selected={t.id === value}
          tabIndex={t.id === value ? 0 : -1}
          onClick={() => onChange(t.id)}
          className={`h-10 shrink-0 px-2 text-[13px] transition-colors duration-[120ms] ease-out ${
            t.id === value ? "font-medium text-fg shadow-[inset_0_-2px_0_var(--color-accent)]" : "text-muted hover:text-fg"
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
  return (
    <div role="group" aria-label={label} className="flex gap-1 rounded-md bg-bg p-0.5">
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          aria-pressed={o.id === value}
          title={disabled ? disabledReason : o.title}
          disabled={disabled}
          onClick={() => onChange(o.id)}
          className={`tabular h-7 flex-1 rounded px-2 text-[12px] transition-colors duration-[120ms] ease-out disabled:cursor-not-allowed disabled:opacity-40 ${
            o.id === value ? "bg-line font-medium text-fg" : "text-muted enabled:hover:text-fg"
          }`}
        >
          {o.label}
        </button>
      ))}
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
  return (
    <button
      type="button"
      aria-pressed={selected}
      disabled={disabled}
      title={title ?? label}
      onClick={onClick}
      className="group flex min-w-0 flex-col gap-1 rounded-md text-left disabled:cursor-not-allowed disabled:opacity-40"
    >
      <span
        className={`relative block aspect-[4/3] w-full overflow-hidden rounded-md border bg-bg ${
          selected ? "border-accent shadow-[0_0_0_1px_var(--color-accent)]" : "border-line group-enabled:group-hover:border-muted"
        }`}
      >
        {children}
        {selected && (
          <span className="absolute right-1 top-1 flex h-4 w-4 items-center justify-center rounded-full bg-accent text-black">
            <Check size={11} strokeWidth={3} />
          </span>
        )}
      </span>
      <span className={`truncate text-[11px] ${selected ? "font-medium text-fg" : "text-muted"}`}>{label}</span>
    </button>
  );
}

/** Determinate when `value` is above 0, otherwise an indeterminate sweep. */
export function ProgressBar({ value, className = "" }: { value: number; className?: string }) {
  const known = value > 0;
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={known ? Math.round(value * 100) : undefined}
      className={`h-1.5 overflow-hidden rounded-full bg-raised ${className}`}
    >
      {known ? (
        <div className="h-full rounded-full bg-accent transition-[width] duration-200" style={{ width: `${Math.max(2, value * 100)}%` }} />
      ) : (
        <div className="indeterminate h-full w-1/3 rounded-full bg-accent" />
      )}
    </div>
  );
}

/** A style preview in the font the engine draws it with. */
export function TextSwatch({ style, label }: { style: TextStyle; label: string }) {
  return (
    <span
      className="inline-block max-w-full truncate rounded px-1.5 text-[15px] leading-6"
      style={{
        fontFamily: fontCss(style.fontFamily),
        color: style.color,
        fontWeight: style.bold ? 700 : 400,
        background: style.background ?? undefined,
        WebkitTextStroke: style.strokeWidth > 0 ? `${Math.min(2, style.strokeWidth / 4)}px ${style.strokeColor}` : undefined,
        paintOrder: "stroke fill",
      }}
    >
      {label}
    </span>
  );
}
