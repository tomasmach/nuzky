import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import { useEditor } from "../lib/store";

/**
 * Bottom-left above the timeline, over the media panel and no wider than it (340 px less 12 px on
 * each side), so they never cover the video frame or the transport.
 */
export function Toasts({ bottom }: { bottom: number }) {
  const toasts = useEditor((s) => s.toasts);
  const lift = useEditor((s) => s.toastLift);
  const dismiss = useEditor((s) => s.dismissToast);
  // An action such as Undo checks the project each render, so it hides once the history moved on.
  useEditor((s) => `${s.snap?.sessionEpoch}:${s.snap?.revision}`);
  return (
    <div className="pointer-events-none fixed left-3 z-[90] flex w-[316px] flex-col items-start gap-2" style={{ bottom: bottom + lift }} aria-live="polite">
      {toasts.map((t) => (
        <div
          key={t.id}
          role={t.kind === "error" ? "alert" : "status"}
          className={`pointer-events-auto flex max-w-full items-center gap-2 rounded-lg border bg-raised px-3 py-2 text-[13px] text-fg shadow-xl shadow-black/50 ${
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
          {t.action && (t.action.valid?.() ?? true) && (
            <button
              type="button"
              className="shrink-0 rounded px-1.5 py-0.5 font-medium text-accent hover:bg-accent/10"
              onClick={() => {
                t.action!.run();
                dismiss(t.id);
              }}
            >
              {t.action.label}
            </button>
          )}
          <button type="button" aria-label="Dismiss" className="shrink-0 rounded text-muted hover:text-fg" onClick={() => dismiss(t.id)}>
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
