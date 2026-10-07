import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Copy, ListChecks, Scissors, Trash2, Unlink } from "lucide-react";
import { AI_EDITING, canSplitClip, deleteSelection, detachAudio, detachBlocker, duplicateSelection, findClip, selectTrack, splitAtPlayhead, useAiLocked, useEditor } from "../../lib/store";
import type { Clip } from "../../lib/types";

export interface MenuAt {
  clipId: string;
  x: number;
  y: number;
}

function Item({ icon, label, shortcut, disabled, reason, onSelect }: { icon: ReactNode; label: string; shortcut?: string; disabled?: boolean; reason?: string | null; onSelect: () => void }) {
  return (
    // Disabled items stay reachable with ↑/↓, as in other menus, so their reason is read out.
    <button
      type="button"
      role="menuitem"
      aria-disabled={disabled || undefined}
      title={disabled ? (reason ?? undefined) : undefined}
      onClick={disabled ? undefined : onSelect}
      className="flex h-8 w-full items-center gap-2 rounded px-2 text-left text-[13px] text-fg hover:bg-raised focus-visible:bg-raised aria-disabled:cursor-not-allowed aria-disabled:text-subtle aria-disabled:hover:bg-transparent aria-disabled:focus-visible:bg-raised"
    >
      <span className="text-muted">{icon}</span>
      <span className="flex-1">{label}</span>
      {shortcut && <span className="text-[12px] text-muted">{shortcut}</span>}
    </button>
  );
}

/**
 * Right-click menu for a clip. Every action applies to the selection, which a right-click outside
 * it replaces with the clicked clip; with several clips selected the labels say how many.
 */
export function ClipMenu({ at, onClose }: { at: MenuAt; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const project = useEditor((s) => s.snap!.project);
  const selection = useEditor((s) => s.selection);
  const [pos, setPos] = useState({ x: at.x, y: at.y });
  const found = findClip(project, at.clipId);
  const picked = selection.map((id) => findClip(project, id)?.clip).filter((c): c is Clip => !!c);
  const locked = useAiLocked();
  const splittable = useEditor((s) => picked.filter((c) => canSplitClip(c, s.timeUs, project.canvas.fps)).length);

  // Focus goes back where it was when the menu opened, unless an action moved it elsewhere.
  const [opener] = useState(() => document.activeElement as HTMLElement | null);
  useLayoutEffect(
    () => () => {
      const active = document.activeElement;
      const lost = !active || active === document.body || !!ref.current?.contains(active);
      if (lost && opener?.isConnected && opener !== document.body) opener.focus({ preventScroll: true });
    },
    [opener],
  );

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    // Keep the menu inside the window.
    const r = el.getBoundingClientRect();
    setPos({ x: Math.min(at.x, window.innerWidth - r.width - 8), y: Math.min(at.y, window.innerHeight - r.height - 8) });
    el.querySelector<HTMLElement>("[role=menuitem]:not([aria-disabled=true])")?.focus();
  }, [at]);

  useEffect(() => {
    const close = (e: Event) => !ref.current?.contains(e.target as Node) && onClose();
    // Esc closes the menu only, wherever focus is, and never also clears the selection.
    const esc = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      onClose();
    };
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("wheel", close, true);
    window.addEventListener("blur", onClose);
    window.addEventListener("keydown", esc, true);
    return () => {
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("wheel", close, true);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("keydown", esc, true);
    };
  }, [onClose]);

  if (!found || picked.length === 0) return null;
  const { clip } = found;
  const n = picked.length;
  const many = (one: string, verb: string) => (n === 1 ? one : `${verb} ${n} clips`);
  const detachable = picked.filter((c) => !detachBlocker(project, c));
  const detachReason = n === 1 ? detachBlocker(project, picked[0]) : "None of the selected clips has sound to detach";
  const run = (fn: () => unknown) => () => {
    onClose();
    fn();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    e.stopPropagation();
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = [...(ref.current?.querySelectorAll<HTMLElement>("[role=menuitem]") ?? [])];
    const i = items.indexOf(document.activeElement as HTMLElement);
    items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
  };

  return (
    <div
      ref={ref}
      role="menu"
      aria-label={n === 1 ? "Clip actions" : `Actions for ${n} clips`}
      onKeyDown={onKeyDown}
      onContextMenu={(e) => e.preventDefault()}
      className="fixed z-[110] w-56 rounded-lg border border-line bg-panel p-1 shadow-2xl shadow-black/60"
      style={{ left: pos.x, top: pos.y }}
    >
      {/* Split takes the selected clips under the playhead, like S. */}
      <Item
        icon={<Scissors size={14} />}
        label={n === 1 || splittable === 0 ? "Split at playhead" : splittable === n ? `Split ${n} clips` : `Split ${splittable} of ${n} clips at playhead`}
        shortcut="S"
        disabled={locked || splittable === 0}
        reason={locked ? AI_EDITING : n === 1 ? "Move the playhead over this clip to split it" : "Move the playhead over the selected clips to split them"}
        onSelect={run(splitAtPlayhead)}
      />
      <Item icon={<Copy size={14} />} label={many("Duplicate", "Duplicate")} shortcut="Ctrl+D" disabled={locked} reason={AI_EDITING} onSelect={run(duplicateSelection)} />
      <Item
        icon={<Unlink size={14} />}
        label={n === 1 || detachable.length === 0 ? "Detach audio" : `Detach audio of ${detachable.length} clip${detachable.length === 1 ? "" : "s"}`}
        disabled={locked || detachable.length === 0}
        reason={locked ? AI_EDITING : detachReason}
        onSelect={run(() => detachAudio(detachable.map((c) => c.id)))}
      />
      <Item icon={<ListChecks size={14} />} label="Select all on track" onSelect={run(() => selectTrack(clip.id))} />
      <div className="my-1 h-px bg-line" />
      <Item icon={<Trash2 size={14} />} label={many("Delete", "Delete")} shortcut="Del" disabled={locked} reason={AI_EDITING} onSelect={run(deleteSelection)} />
    </div>
  );
}
