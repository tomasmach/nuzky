import { useEffect, useRef, useState, type KeyboardEvent, type MouseEvent, type PointerEvent } from "react";
import { Check, Ellipsis } from "lucide-react";
import { blocked, renameProject, useLibrary, whenLabel } from "../../lib/library";
import { useEditor } from "../../lib/store";
import type { LibraryProject } from "../../lib/types";
import { Poster } from "./Poster";

/** Width over height of a card's picture box: room for a 9:16 video at full height. */
const CARD_BOX = 152 / 180;

/** The line under the name: when, and its collection; or what keeps it from opening. */
function meta(p: LibraryProject, open: boolean, collection: string | undefined) {
  if (p.state === "busy") return "In use elsewhere";
  if (p.state === "broken") return "Damaged file";
  const when = open ? "Open now" : whenLabel(p.modifiedMs);
  return collection ? `${when} · ${collection}` : when;
}

export function ProjectCard({
  p,
  focusable,
  selected,
  menuOpen,
  selecting,
  onOpen,
  onToggle,
  onMenu,
  onDragStart,
}: {
  p: LibraryProject;
  /** The one card the grid's Tab stop lands on. */
  focusable: boolean;
  selected: boolean;
  /** Its menu is open: the card is marked as what the menu acts on. */
  menuOpen: boolean;
  /** Something is selected, so a click selects instead of opening. */
  selecting: boolean;
  onOpen: () => void;
  onToggle: (e: { shiftKey: boolean }) => void;
  onMenu: (at: { x: number; y: number }, keyboard: boolean) => void;
  onDragStart: (e: PointerEvent<HTMLDivElement>) => void;
}) {
  const open = useEditor((s) => s.snap?.path === p.path);
  const ai = useEditor((s) => open && !!s.aiRun);
  const collection = useLibrary((s) => s.collections.find((c) => c.id === p.collection)?.name);
  const renaming = useLibrary((s) => s.renaming === p.path);
  const reason = blocked(p);

  const onClick = (e: MouseEvent) => {
    if ((e.target as Element).closest("[data-card-button]")) return;
    if (e.ctrlKey || e.metaKey || e.shiftKey || selecting) onToggle(e);
    else onOpen();
  };

  return (
    <div
      role="option"
      aria-selected={selected}
      aria-disabled={reason ? true : undefined}
      aria-label={`${p.name}, ${meta(p, open, collection)}`}
      title={reason ?? undefined}
      data-path={p.path}
      tabIndex={focusable ? 0 : -1}
      onClick={onClick}
      onPointerDown={(e) => e.button === 0 && !(e.target as Element).closest("[data-card-button], input") && onDragStart(e)}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu({ x: e.clientX, y: e.clientY }, false);
      }}
      className="group/card relative flex min-w-0 cursor-default flex-col outline-none"
    >
      <div
        className={`relative w-full overflow-hidden rounded-[10px] bg-stage outline transition-[outline-color] duration-[120ms] ease-out ${
          selected
            ? "outline-2 outline-offset-0 outline-accent group-focus-visible/card:outline-offset-2"
            : menuOpen
              ? "outline-2 outline-offset-2 outline-accent"
              : "outline-1 -outline-offset-1 outline-white/[.06] group-hover/card:outline-white/[.16] group-focus-visible/card:outline-2 group-focus-visible/card:outline-offset-2 group-focus-visible/card:outline-accent"
        }`}
        style={{ aspectRatio: `${CARD_BOX}` }}
      >
        <Poster p={p} box={CARD_BOX} ai={ai} />
        <button
          type="button"
          tabIndex={-1}
          aria-hidden
          data-card-button
          title={selected ? "Deselect" : "Select"}
          onClick={(e) => onToggle(e)}
          className={`absolute left-1.5 top-1.5 flex h-5 w-5 items-center justify-center rounded-full transition-opacity duration-[120ms] ${
            selected
              ? "bg-accent-strong text-white"
              : `border-[1.5px] border-white/70 bg-black/40 text-transparent ${selecting ? "opacity-100" : "opacity-0 group-hover/card:opacity-100 group-focus-visible/card:opacity-100"}`
          }`}
        >
          <Check size={12} strokeWidth={3} />
        </button>
        <button
          type="button"
          tabIndex={-1}
          aria-hidden
          data-card-button
          title="More"
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            onMenu({ x: r.left, y: r.bottom + 4 }, false);
          }}
          className={`absolute right-1.5 top-1.5 flex h-6 w-6 items-center justify-center rounded-full bg-black/55 text-fg transition-opacity duration-[120ms] hover:bg-black/75 group-hover/card:opacity-100 group-focus-visible/card:opacity-100 ${menuOpen ? "opacity-100" : "opacity-0"}`}
        >
          <Ellipsis size={14} />
        </button>
      </div>
      {renaming ? (
        <RenameField p={p} />
      ) : (
        <div data-name className="mt-2 truncate text-[13px] font-medium text-fg">
          {p.name}
        </div>
      )}
      <div data-meta className={`tabular mt-0.5 truncate text-[12px] ${p.state === "broken" ? "text-danger" : "text-muted"}`}>
        {meta(p, open, collection)}
      </div>
    </div>
  );
}

/** The name as a field: Enter or clicking elsewhere renames, Esc keeps the old name. */
function RenameField({ p }: { p: LibraryProject }) {
  const [draft, setDraft] = useState(p.name);
  const input = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  const finish = (save: boolean) => {
    if (done.current) return;
    done.current = true;
    useLibrary.setState({ renaming: null });
    if (save) void renameProject(p, draft);
    // Focus goes back to the card, so the keyboard stays in the grid.
    requestAnimationFrame(() => document.querySelector<HTMLElement>(`[data-path="${CSS.escape(p.path)}"]`)?.focus());
  };
  return (
    <input
      ref={input}
      aria-label="Project name"
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => finish(true)}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(true);
        if (e.key === "Escape") finish(false);
      }}
      className="mt-1.5 h-6 w-full rounded-md border border-accent bg-white/[.06] px-1.5 text-[13px] font-medium text-fg outline-none"
    />
  );
}
