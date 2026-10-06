import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Copy, Scissors, Trash2, Unlink } from "lucide-react";
import { deleteSelection, detachAudio, detachBlocker, duplicateSelection, findClip, useEditor } from "../../lib/store";
import { US } from "../../lib/time";

export interface MenuAt {
  clipId: string;
  x: number;
  y: number;
}

function Item({ icon, label, shortcut, disabled, reason, onSelect }: { icon: ReactNode; label: string; shortcut?: string; disabled?: boolean; reason?: string | null; onSelect: () => void }) {
  return (
    <button
      type="button"
      role="menuitem"
      disabled={disabled}
      title={disabled ? (reason ?? undefined) : undefined}
      onClick={onSelect}
      className="flex h-8 w-full items-center gap-2 rounded px-2 text-left text-[13px] text-fg hover:bg-raised focus-visible:bg-raised disabled:cursor-not-allowed disabled:text-subtle disabled:hover:bg-transparent"
    >
      <span className="text-muted">{icon}</span>
      <span className="flex-1">{label}</span>
      {shortcut && <span className="text-[12px] text-muted">{shortcut}</span>}
    </button>
  );
}

/** Right-click menu for a clip; the clip is already selected when it opens. */
export function ClipMenu({ at, onClose }: { at: MenuAt; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const project = useEditor((s) => s.snap!.project);
  const timeUs = useEditor((s) => s.timeUs);
  const [pos, setPos] = useState({ x: at.x, y: at.y });
  const found = findClip(project, at.clipId);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    // Keep the menu inside the window.
    const r = el.getBoundingClientRect();
    setPos({ x: Math.min(at.x, window.innerWidth - r.width - 8), y: Math.min(at.y, window.innerHeight - r.height - 8) });
    el.querySelector<HTMLElement>("[role=menuitem]:not(:disabled)")?.focus();
  }, [at]);

  useEffect(() => {
    const close = (e: Event) => !ref.current?.contains(e.target as Node) && onClose();
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("wheel", close, true);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("wheel", close, true);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);

  if (!found) return null;
  const { clip } = found;
  const min = US / project.canvas.fps;
  const splittable = timeUs > clip.startUs + min && timeUs < clip.startUs + clip.durationUs - min;
  const blocker = detachBlocker(project, clip);
  const run = (fn: () => unknown) => () => {
    onClose();
    fn();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    e.stopPropagation();
    if (e.key === "Escape") onClose();
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = [...(ref.current?.querySelectorAll<HTMLElement>("[role=menuitem]:not(:disabled)") ?? [])];
    const i = items.indexOf(document.activeElement as HTMLElement);
    items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
  };

  return (
    <div
      ref={ref}
      role="menu"
      aria-label="Clip actions"
      onKeyDown={onKeyDown}
      onContextMenu={(e) => e.preventDefault()}
      className="fixed z-[110] w-56 rounded-lg border border-line bg-panel p-1 shadow-2xl shadow-black/60"
      style={{ left: pos.x, top: pos.y }}
    >
      <Item
        icon={<Scissors size={14} />}
        label="Split at playhead"
        shortcut="S"
        disabled={!splittable}
        reason="Move the playhead over this clip to split it"
        onSelect={run(() => useEditor.getState().edit({ type: "splitClip", clipId: clip.id, atUs: Math.round(timeUs) }))}
      />
      <Item icon={<Copy size={14} />} label="Duplicate" shortcut="Ctrl+D" onSelect={run(duplicateSelection)} />
      <Item icon={<Unlink size={14} />} label="Detach audio" disabled={!!blocker} reason={blocker} onSelect={run(() => detachAudio(clip.id))} />
      <div className="my-1 h-px bg-line" />
      <Item icon={<Trash2 size={14} />} label="Delete" shortcut="Del" onSelect={run(deleteSelection)} />
    </div>
  );
}
