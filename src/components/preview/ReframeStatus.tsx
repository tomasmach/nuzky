import { Loader2 } from "lucide-react";
import { api } from "../../lib/api";
import { useReframe } from "../../lib/reframe";
import { undoAction, useEditor } from "../../lib/store";
import { Button } from "../ui";

/**
 * Top left of the preview area, in the cover's status chip: the reframe's progress with Stop, then what it did with
 * Undo while it is still the newest step, and Done. Beside a vertical frame it sits on the stage, clear of the video.
 */
export function ReframeStatus() {
  const id = useReframe((s) => s.job);
  const done = useReframe((s) => s.done);
  const job = useEditor((s) => (id ? s.jobs[id] : undefined));
  // Undo applies only while the reframe is the newest step; any later edit hides it.
  useEditor((s) => s.snap);
  const chip = "absolute left-2 top-2 z-20 flex h-7 max-w-[calc(100%-16px)] items-center gap-1.5 rounded-lg bg-black/70 pl-2 pr-1 text-[12px] text-fg";
  const small = "h-5 shrink-0 px-2 text-[11px]";
  if (id)
    return (
      <div className={chip} role="status">
        <Loader2 size={13} className="shrink-0 animate-spin text-accent" />
        <span className="tabular truncate">
          {job?.phase ?? "Following the face"}
          {job && job.progress > 0 ? ` ${Math.round(job.progress * 100)}%` : "…"}
        </span>
        <Button pill className={small} onClick={() => void api.cancelJob(id)}>
          Stop
        </Button>
      </div>
    );
  if (!done) return null;
  const undo = undoAction(done.snap);
  if (!undo.valid?.()) return null;
  const close = () => useReframe.setState({ done: null });
  return (
    <div className={chip} role="status">
      <span className="truncate">{done.text}</span>
      <Button
        pill
        className={small}
        onClick={() => {
          close();
          undo.run();
        }}
      >
        Undo
      </Button>
      <Button pill className={small} onClick={close}>
        Done
      </Button>
    </div>
  );
}
