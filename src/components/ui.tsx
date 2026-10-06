import type { ButtonHTMLAttributes, ReactNode } from "react";

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

/** Icon-only button. `label` is required: it becomes the tooltip and the accessible name. */
export function IconButton({
  label,
  active = false,
  className = "",
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={active || undefined}
      className={`inline-flex h-8 w-8 items-center justify-center rounded-md transition-colors duration-[120ms] ease-out active:translate-y-px disabled:cursor-not-allowed disabled:opacity-35 disabled:active:translate-y-0 ${
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
      {hint && <span className="text-[11px] text-subtle">{hint}</span>}
    </label>
  );
}

/** Slider with a numeric readout that can also be typed into. */
export function Slider({
  label,
  value,
  min,
  max,
  step,
  unit = "",
  onChange,
  format = (v) => String(Math.round(v * 100) / 100),
  parse = Number,
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
}) {
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between">
        <span className="text-[12px] text-muted">{label}</span>
        <span className="flex items-center gap-1">
          <input
            aria-label={`${label} value`}
            className="tabular h-6 w-14 rounded border border-line bg-raised px-1.5 text-right text-[12px] text-fg"
            value={format(value)}
            onChange={(e) => {
              const v = parse(e.target.value);
              if (Number.isFinite(v)) onChange(Math.max(min, Math.min(max, v)));
            }}
          />
          {unit && <span className="w-3 text-[11px] text-subtle">{unit}</span>}
        </span>
      </div>
      <input
        type="range"
        aria-label={label}
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className="w-full"
      />
    </div>
  );
}

export function Section({ title, children, actions }: { title: string; children: ReactNode; actions?: ReactNode }) {
  return (
    <section className="flex flex-col gap-3 border-b border-line px-4 py-4 last:border-b-0">
      <div className="flex items-center justify-between">
        <h3 className="text-[12px] font-semibold uppercase tracking-wide text-muted">{title}</h3>
        {actions}
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
        <span className="tabular text-[12px] text-subtle">{value.slice(0, 7).toUpperCase()}</span>
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

export function ProgressBar({ value, className = "" }: { value: number; className?: string }) {
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(value * 100)}
      className={`h-1.5 overflow-hidden rounded-full bg-raised ${className}`}
    >
      <div className="h-full rounded-full bg-accent transition-[width] duration-200" style={{ width: `${Math.max(2, value * 100)}%` }} />
    </div>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="rounded border border-line bg-raised px-1 text-[10px] text-muted">{children}</kbd>;
}
