import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { AlertCircle, AlertTriangle, CheckCircle2, Info, X } from "lucide-react";
import { useEditor, type Toast } from "../lib/store";

/** How long a toast stays, in ms; errors, warnings and sticky notices stay until dismissed. */
const lifetime = (t: Toast) => (t.kind === "error" || t.kind === "warning" || t.sticky ? null : t.action ? 7000 : 5000);

/** The `overlay` shadow with the hairline in the toast's colour; a border would double the hairline. */
const EDGE = {
  error: "shadow-[inset_0_1px_0_rgb(255_255_255/.12),inset_0_0_0_1px_rgb(255_69_58/.55),0_18px_50px_rgb(0_0_0/.55)]",
  warning: "shadow-[inset_0_1px_0_rgb(255_255_255/.12),inset_0_0_0_1px_rgb(255_159_10/.55),0_18px_50px_rgb(0_0_0/.55)]",
} as const;

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
  // A toast about an action that no longer applies, such as Undo after another change, goes.
  const expired = !!t.action && !action;
  useEffect(() => {
    if (expired) onDismiss();
  }, [expired, onDismiss]);
  return (
    <div
      data-toast
      tabIndex={-1}
      role={t.kind === "error" ? "alert" : undefined}
      // Only the keyboard (F8, Esc) focuses a toast, so it always shows the focus outline; a click on its text does not focus it.
      onMouseDown={(e) => !(e.target as Element).closest("button") && e.preventDefault()}
      className={`overlay pointer-events-auto flex min-h-12 max-w-full items-center gap-2.5 rounded-[14px] py-2 pr-2 pl-3.5 text-[13px] font-medium text-fg focus:outline-2 focus:outline-offset-1 focus:outline-accent ${
        t.kind === "error" || t.kind === "warning" ? EDGE[t.kind] : ""
      }`}
    >
      {t.kind === "error" ? (
        <AlertCircle size={18} className="shrink-0 text-danger" />
      ) : t.kind === "warning" ? (
        <AlertTriangle size={18} className="shrink-0 text-warn" />
      ) : t.kind === "success" ? (
        <CheckCircle2 size={18} className="shrink-0 text-ok" />
      ) : (
        <Info size={18} className="shrink-0 text-muted" />
      )}
      <span className="min-w-0 flex-1 break-words">{t.text}</span>
      {action && (
        <button
          type="button"
          className="shrink-0 rounded-md px-1.5 py-1 font-semibold text-accent transition-colors duration-[120ms] ease-out hover:bg-white/[.08]"
          onClick={() => {
            action.run();
            onDismiss();
          }}
        >
          {action.label}
        </button>
      )}
      <button
        type="button"
        aria-label="Dismiss"
        title="Dismiss (Esc)"
        className="flex h-[26px] w-[26px] shrink-0 items-center justify-center rounded-full text-muted transition-colors duration-[120ms] ease-out hover:bg-white/[.08] hover:text-fg"
        onClick={onDismiss}
      >
        <X size={14} />
      </button>
    </div>
  );
}

/**
 * Bottom-left above the timeline, over the library and no wider than it (360 px less 12 px on
 * each side), so they never cover the video frame or the transport. With the AI panel docked on
 * the left, they move right with the library. The stack is a polite live
 * region; errors are alerts. F8 moves focus to the newest toast, Esc there dismisses it.
 */
export function Toasts({ bottom, left }: { bottom: number; left: number }) {
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
      className="pointer-events-none fixed z-[90] flex w-[336px] flex-col items-start gap-2"
      style={{ bottom: bottom + lift, left }}
    >
      {toasts.map((t) => (
        <ToastItem key={t.id} toast={t} paused={hovered || focused} dismiss={dismiss} />
      ))}
    </div>
  );
}
