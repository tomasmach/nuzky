import { useRef } from "react";
import { Loader2 } from "lucide-react";
import { api } from "../../lib/api";
import { useReframe } from "../../lib/reframe";
import { undoAction, useEditor } from "../../lib/store";
import { Button } from "../ui";

/**
 * Bottom middle of the preview area, above the transport, in the cover's status chip: the reframe's progress with
 * Stop, then what it did with Undo while it is still the newest step, and Done.
 */
export function ReframeStatus() {
  const id = useReframe((s) => s.job);
  const done = useReframe((s) => s.done);
  const job = useEditor((s) => (id ? s.jobs[id] : undefined));
  // Undo applies only while the reframe is the newest step; any later edit hides it.
  useEditor((s) => s.snap);
  const ref = useRef<HTMLDivElement>(null);
  // The chip goes away under the keyboard's focus; it returns to Ratio, where Reframe was chosen.
  const leave = () => {
    if (ref.current?.contains(document.activeElement)) document.querySelector<HTMLElement>("[data-ratio]")?.focus();
  };
  const chip =
    "absolute bottom-2 left-1/2 z-20 flex h-7 max-w-[calc(100%-16px)] -translate-x-1/2 items-center gap-1.5 whitespace-nowrap rounded-lg bg-black/70 pl-2 pr-1 text-[12px] text-fg";
  const small = "h-5 shrink-0 px-2 text-[11px]";
  if (id)
    return (
      <div ref={ref} className={chip} role="status">
        <Loader2 size={13} className="shrink-0 animate-spin text-accent" />
        <span className="tabular truncate">
          {job?.phase ?? "Following the face"}
          {job && job.progress > 0 ? ` · ${Math.round(job.progress * 100)}%` : "…"}
        </span>
        <Button
          pill
          className={small}
          onClick={() => {
            leave();
            void api.cancelJob(id);
          }}
        >
          Stop
        </Button>
      </div>
    );
  if (!done) return null;
  const undo = undoAction(done.snap);
  if (!undo.valid?.()) return null;
  const close = () => {
    leave();
    useReframe.setState({ done: null });
  };
  return (
    <div ref={ref} className={chip} role="status">
      <span className="truncate" title={done.text}>
        {done.text}
      </span>
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
