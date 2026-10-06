import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import { useEditor } from "../lib/store";

/** Bottom-left above the timeline, over the media panel, so they never cover the video frame. */
export function Toasts({ bottom }: { bottom: number }) {
  const toasts = useEditor((s) => s.toasts);
  const dismiss = useEditor((s) => s.dismissToast);
  return (
    <div className="pointer-events-none fixed left-3 z-[90] flex w-[360px] flex-col items-start gap-2" style={{ bottom }} aria-live="polite">
      {toasts.map((t) => (
        <div
          key={t.id}
          role={t.kind === "error" ? "alert" : "status"}
          className={`pointer-events-auto flex max-w-full items-center gap-2 rounded-lg border px-3 py-2 text-[13px] shadow-xl shadow-black/50 ${
            t.kind === "error" ? "border-danger/50 bg-[#2a1416] text-fg" : "border-line bg-raised text-fg"
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
          {t.action && (
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
