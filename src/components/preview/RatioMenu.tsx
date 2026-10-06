import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, RectangleHorizontal } from "lucide-react";
import { FORMATS, formatLabel } from "../../lib/presets";
import { useEditor } from "../../lib/store";

/** CapCut's "Ratio" control next to the player: the one place to change the canvas format. */
export function RatioMenu() {
  const canvas = useEditor((s) => s.snap!.project.canvas);
  const edit = useEditor((s) => s.edit);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    ref.current?.querySelector<HTMLElement>("[aria-checked=true]")?.focus();
    const close = (e: PointerEvent) => !ref.current?.contains(e.target as Node) && setOpen(false);
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
        ref.current?.querySelector<HTMLElement>("button")?.focus();
      }
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", key, true);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", key, true);
    };
  }, [open]);

  return (
    <div className="relative" ref={ref}>
      <button
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        title="Canvas ratio"
        onClick={() => setOpen(!open)}
        className={`flex h-8 items-center gap-1.5 rounded-md border px-2 text-[12px] transition-colors duration-[120ms] ease-out ${
          open ? "border-muted bg-raised text-fg" : "border-line text-fg hover:bg-raised"
        }`}
      >
        <RectangleHorizontal size={14} className="text-muted" />
        <span className="text-muted">Ratio</span>
        <span className="tabular font-medium">{formatLabel(canvas.width, canvas.height)}</span>
        <ChevronDown size={13} className="text-muted" />
      </button>
      {open && (
        <div role="menu" aria-label="Canvas ratio" className="absolute bottom-10 right-0 z-50 w-60 rounded-lg border border-line bg-panel p-1 shadow-2xl shadow-black/60">
          {FORMATS.map((f) => {
            const on = f.width === canvas.width && f.height === canvas.height;
            return (
              <button
                key={f.label}
                type="button"
                role="menuitemradio"
                aria-checked={on}
                onClick={() => {
                  if (!on) edit({ type: "setCanvas", width: f.width, height: f.height });
                  setOpen(false);
                }}
                className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left hover:bg-raised"
              >
                <span className="tabular w-9 text-[13px] font-medium text-fg">{f.label}</span>
                <span className="flex-1 text-[12px] text-muted">{f.hint}</span>
                {on && <Check size={14} className="text-accent" />}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
