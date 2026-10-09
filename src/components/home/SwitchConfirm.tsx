import { useEffect, useRef } from "react";
import { Sparkles } from "lucide-react";
import { resolveSwitch, useLibrary, type PendingSwitch } from "../../lib/library";
import { useEditor } from "../../lib/store";
import { Button, trapTab } from "../ui";

/**
 * What happens to the agent's changes when another project opens during its run. The choice is made
 * here, because the open project's undo history stays behind when it closes.
 */
export function SwitchChoice({ pending, dialog = false }: { pending: PendingSwitch; /** Titled like a dialog rather than a panel. */ dialog?: boolean }) {
  const name = useEditor((s) => s.snap?.project.name ?? "this project");
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => box.current?.querySelector<HTMLButtonElement>("[data-autofocus]")?.focus(), []);
  return (
    <div ref={box} className="flex flex-col gap-2">
      <h2 id="switch-title" className={`flex items-center gap-2 font-semibold text-fg ${dialog ? "text-[17px]" : "text-[13px]"}`}>
        <Sparkles size={dialog ? 17 : 14} className="shrink-0 text-accent" />
        Stop the AI edit and {pending.what}?
      </h2>
      <p id="switch-description" className="text-[13px] leading-[18px] text-muted">
        AI is editing “{name}”. Choose what happens to its changes; after you switch they can't be undone. Your agent will need to reconnect to Nuzky.
      </p>
      <div className="mt-2 flex justify-end gap-2">
        <Button variant="ghost" pill onClick={() => void resolveSwitch("cancel")}>
          Cancel
        </Button>
        {/* It throws the agent's work away for good, so it reads as destructive. */}
        <Button variant="danger" pill onClick={() => void resolveSwitch("undo")}>
          Undo changes and {pending.verb}
        </Button>
        <Button data-autofocus variant="primary" pill onClick={() => void resolveSwitch("keep")}>
          Keep changes and {pending.verb}
        </Button>
      </div>
    </div>
  );
}

/** The same choice as a dialog, where no launcher is open to hold it. */
export function SwitchDialog() {
  const pending = useLibrary((s) => s.pendingSwitch);
  const launcherOpen = useEditor((s) => s.launcherOpen);
  const dialog = useRef<HTMLDialogElement>(null);
  const shown = !!pending && !launcherOpen;
  useEffect(() => {
    if (shown && !dialog.current?.open) dialog.current?.showModal();
  }, [shown]);
  if (!shown) return null;
  return (
    <dialog
      ref={dialog}
      aria-labelledby="switch-title"
      aria-describedby="switch-description"
      onCancel={(e) => {
        e.preventDefault();
        void resolveSwitch("cancel");
      }}
      onKeyDown={trapTab}
      className="dialog-in overlay fixed inset-0 m-auto w-[500px] rounded-[20px] p-6 text-fg backdrop:bg-black/45"
    >
      <SwitchChoice pending={pending} dialog />
    </dialog>
  );
}
