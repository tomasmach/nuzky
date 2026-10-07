import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import { useEditor, type Toast } from "../lib/store";

/** How long a toast stays, in ms; an error stays until it is dismissed. */
const lifetime = (t: Toast) => (t.kind === "error" ? null : t.action ? 7000 : 5000);

/** One toast. Its time runs only while the stack is neither hovered nor focused. */
function ToastItem({ toast: t, paused, dismiss }: { toast: Toast; paused: boolean; dismiss: (id: number) => void }) {
  const onDismiss = useCallback(() => dismiss(t.id), [dismiss, t.id]);
  const left = useRef(lifetime(t));
  useEffect(() => {
    if (paused || left.current === null) return;
    const started = Date.now();
    const timer = window.setTimeout(onDismiss, left.current);
    return () => {
      window.clearTimeout(timer);
      if (left.current !== null) left.current -= Date.now() - started;
    };
  }, [paused, onDismiss]);
  const action = t.action && (t.action.valid?.() ?? true) ? t.action : null;
  return (
    <div
      data-toast
      tabIndex={-1}
      role={t.kind === "error" ? "alert" : undefined}
      // Only the keyboard (F8, Esc) focuses a toast, so it always shows the focus outline; a click on its text does not focus it.
      onMouseDown={(e) => !(e.target as Element).closest("button") && e.preventDefault()}
      className={`pointer-events-auto flex max-w-full items-center gap-2 rounded-lg border bg-raised px-3 py-2 text-[13px] text-fg shadow-xl shadow-black/50 focus:outline-2 focus:outline-offset-1 focus:outline-accent ${
        t.kind === "error" ? "border-danger/50" : "border-line"
      }`}
    >
      {t.kind === "error" ? (
        <AlertCircle size={16} className="shrink-0 text-danger" />
      ) : t.kind === "success" ? (
        <CheckCircle2 size={16} className="shrink-0 text-accent" />
      ) : (
        <Info size={16} className="shrink-0 text-muted" />
      )}
      <span className="min-w-0 break-words">{t.text}</span>
      {action && (
        <button
          type="button"
          className="shrink-0 rounded px-1.5 py-0.5 font-medium text-accent hover:bg-accent/10"
          onClick={() => {
            action.run();
            onDismiss();
          }}
        >
          {action.label}
        </button>
      )}
      <button type="button" aria-label="Dismiss" title="Dismiss (Esc)" className="shrink-0 rounded text-muted hover:text-fg" onClick={onDismiss}>
        <X size={14} />
      </button>
    </div>
  );
}

/**
 * Bottom-left above the timeline, over the media panel and no wider than it (340 px less 12 px on
 * each side), so they never cover the video frame or the transport. The stack is a polite live
 * region; errors are alerts. F8 moves focus to the newest toast, Esc there dismisses it.
 */
export function Toasts({ bottom }: { bottom: number }) {
  const toasts = useEditor((s) => s.toasts);
  const lift = useEditor((s) => s.toastLift);
  const dismiss = useEditor((s) => s.dismissToast);
  // An action such as Undo checks the project each render, so it hides once the history moved on.
  useEditor((s) => `${s.snap?.sessionEpoch}:${s.snap?.revision}`);
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const stack = useRef<HTMLDivElement>(null);
  /** Where focus was before F8, so it goes back there once the toasts are done with. */
  const back = useRef<HTMLElement | null>(null);

  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "F8") return;
      const newest = [...(stack.current?.querySelectorAll<HTMLElement>("[data-toast]") ?? [])].pop();
      if (!newest) return;
      e.preventDefault();
      if (!stack.current?.contains(document.activeElement)) back.current = document.activeElement as HTMLElement | null;
      newest.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Esc dismisses the focused toast; focus moves to the next one, or back to where it came from.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Escape") return;
    const item = (e.target as HTMLElement).closest<HTMLElement>("[data-toast]");
    const t = item && toasts[[...(stack.current?.querySelectorAll("[data-toast]") ?? [])].indexOf(item)];
    if (!t) return;
    e.stopPropagation();
    const rest = [...(stack.current?.querySelectorAll<HTMLElement>("[data-toast]") ?? [])].filter((el) => el !== item);
    dismiss(t.id);
    const next = rest.pop() ?? back.current;
    next?.focus();
  };

  return (
    <div
      ref={stack}
      role="region"
      aria-label="Notifications"
      aria-live="polite"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onFocus={() => setFocused(true)}
      onBlur={(e) => !e.currentTarget.contains(e.relatedTarget as Node | null) && setFocused(false)}
      onKeyDown={onKeyDown}
      className="pointer-events-none fixed left-3 z-[90] flex w-[316px] flex-col items-start gap-2"
      style={{ bottom: bottom + lift }}
    >
      {toasts.map((t) => (
        <ToastItem key={t.id} toast={t} paused={hovered || focused} dismiss={dismiss} />
      ))}
    </div>
  );
}
