import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode, type Ref } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  AlertCircle,
  ArrowUpDown,
  ChevronDown,
  ChevronLeft,
  Copy,
  Folder,
  FolderInput,
  FolderOpen,
  FolderPlus,
  Film,
  Info,
  LayoutGrid,
  Loader2,
  Pencil,
  Plus,
  Search,
  Sparkles,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { followPointer } from "../../lib/drag";
import {
  blocked,
  createCollection,
  deleteCollection,
  duplicateProject,
  fold,
  matches,
  moveToCollection,
  newProject,
  openProject,
  openProjectFile,
  onlyPristine,
  periodOf,
  pickVideosForNewProject,
  refreshLibrary,
  renameCollection,
  setSort,
  showEditor,
  trashProjects,
  undoLastToast,
  useLibrary,
  useSaidSearch,
} from "../../lib/library";
import { FORMATS } from "../../lib/presets";
import { useAgent } from "../../lib/agent";
import { useDock } from "../../lib/dock";
import { useEditor } from "../../lib/store";
import { formatLength } from "../../lib/time";
import type { Collection, LibraryProject, Said } from "../../lib/types";
import { AiRunBar, JobIndicator } from "../TopBar";
import { Button, IconButton, Menu, type MenuEntry } from "../ui";
import { ProjectCard } from "./ProjectCard";
import { Poster } from "./Poster";
import { VersionRow } from "./VersionRow";

type MenuState = { items: MenuEntry[]; at: { x: number; y: number; align?: "start" | "end"; above?: boolean }; label: string; keyboard: boolean; back: HTMLElement | null; /** The card it acts on. */ target?: string };

/** A ratio's shape, as on the format tiles and in the New project menu. */
export function FormatShape({ width, height, size }: { width: number; height: number; size: number }) {
  const scale = size / Math.max(width, height);
  return <span className="inline-block rounded-[2px] border-[1.5px] border-current" style={{ width: Math.round(width * scale), height: Math.round(height * scale) }} />;
}

function newProjectItems(): MenuEntry[] {
  return [
    ...FORMATS.map((f) => ({ label: f.label, shortcut: f.hint, icon: <FormatShape width={f.width} height={f.height} size={14} />, run: () => newProject(f.width, f.height) })),
    "separator" as const,
    { label: "From videos…", shortcut: "First video's format", icon: <Film size={15} />, run: () => void pickVideosForNewProject() },
  ];
}

const byName = new Intl.Collator("en", { numeric: true, sensitivity: "base" });

/** The AI panel works on the open project, so it opens in the editor. */
function openAi() {
  showEditor();
  useDock.setState({ open: true });
}

function AiButton() {
  const working = useAgent((s) => s.status !== "idle");
  return (
    <Button variant="bar" pill title={working ? "Open the editor with AI (Ctrl+J). It is still working." : "Open the editor with AI (Ctrl+J)"} onClick={openAi}>
      {working ? <Loader2 size={15} className="animate-spin text-accent" /> : <Sparkles size={15} />} AI
    </Button>
  );
}

export function Home() {
  const projects = useLibrary((s) => s.projects);
  const collections = useLibrary((s) => s.collections);
  const filter = useLibrary((s) => s.filter);
  const sort = useLibrary((s) => s.sort);
  const selection = useLibrary((s) => s.selection);
  const loadError = useLibrary((s) => s.loadError);
  const current = useEditor((s) => s.snap);
  const fileDrag = useEditor((s) => s.fileDrag);
  const [query, setQuery] = useState("");
  const said = useSaidSearch(query, filter === "all" ? null : filter);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [drag, setDrag] = useState<{ paths: string[]; x: number; y: number; over: string | null } | null>(null);
  const searchField = useRef<HTMLInputElement>(null);
  const grid = useRef<HTMLDivElement>(null);
  const [focusPath, setFocusPath] = useState<string | null>(null);
  const [creating, setCreating] = useState<string[] | null>(null);
  const draggedAt = useRef(0);

  useEffect(() => {
    void refreshLibrary();
    // Files may have changed while CapOpen was in the background; a list a few seconds old is kept.
    const onFocus = () => void refreshLibrary(3000);
    window.addEventListener("focus", onFocus);
    // A search started in the launcher continues here.
    const handed = useLibrary.getState().handoffQuery;
    if (handed) {
      setQuery(handed);
      useLibrary.setState({ handoffQuery: null });
    }
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  const collectionOf = (id: string | null) => collections.find((c) => c.id === id);
  const inView = useMemo(() => (projects ?? []).filter((p) => filter === "all" || p.collection === filter), [projects, filter]);
  const sorted = useMemo(() => (sort === "name" ? [...inView].sort((a, b) => byName.compare(a.name, b.name)) : inView), [inView, sort]);
  const searching = query.trim().length > 0;
  const found = useMemo(() => (searching ? sorted.filter((p) => matches(p, query, collections)) : sorted), [sorted, searching, query, collections]);
  const firstRun = filter === "all" && !searching && onlyPristine(projects, current);
  const hits = useMemo(
    () => (said.result && said.query === query ? said.result.hits.filter((h) => inView.some((p) => p.path === h.path)) : []),
    [said, query, inView],
  );

  // Focus follows a card asked for, such as a new duplicate, once it is listed.
  const focusAsked = useLibrary((s) => s.focusPath);
  useEffect(() => {
    if (!focusAsked || !found.some((p) => p.path === focusAsked)) return;
    setFocusPath(focusAsked);
    useLibrary.setState({ focusPath: null });
    requestAnimationFrame(() => {
      const card = cardEl(focusAsked);
      card?.focus();
      card?.scrollIntoView({ block: "nearest" });
    });
  }, [focusAsked, found]);

  const tabStop = found.some((p) => p.path === focusPath) ? focusPath : (found.find((p) => p.path === current?.path) ?? found[0])?.path;
  const cardEl = (path: string) => grid.current?.querySelector<HTMLElement>(`[data-path="${CSS.escape(path)}"]`) ?? null;
  /** Enter and ↓ in the search field move to the first result, so what opens next is in plain view. */
  const focusFirstResult = () => {
    const first = grid.current?.querySelector<HTMLElement>("[data-path]") ?? document.querySelector<HTMLElement>('section[aria-label="Said in your videos"] button');
    if (first?.dataset.path) setFocusPath(first.dataset.path);
    first?.focus();
    first?.scrollIntoView({ block: "nearest" });
  };

  const select = (paths: string[]) => useLibrary.setState({ selection: paths });
  const toggle = (p: LibraryProject, range: boolean) => {
    const sel = useLibrary.getState().selection;
    if (range && sel.length > 0) {
      const from = found.findIndex((x) => x.path === sel[sel.length - 1]);
      const to = found.findIndex((x) => x.path === p.path);
      const span = found.slice(Math.min(from, to), Math.max(from, to) + 1).map((x) => x.path);
      select([...new Set([...sel, ...span])]);
    } else select(sel.includes(p.path) ? sel.filter((x) => x !== p.path) : [...sel, p.path]);
    setFocusPath(p.path);
  };
  const open = (p: LibraryProject) => {
    if (Date.now() - draggedAt.current < 300) return;
    const reason = blocked(p);
    if (reason) return useEditor.getState().toast({ kind: "info", text: reason });
    openProject(p);
  };

  const showMenu = (items: MenuEntry[], at: MenuState["at"], label: string, keyboard: boolean, target?: string) =>
    setMenu({ items, at, label, keyboard, target, back: document.activeElement instanceof HTMLElement ? document.activeElement : null });
  const closeMenu = (chose: boolean) => {
    const back = menu?.back;
    setMenu(null);
    if (!chose) back?.focus();
    // A chosen item may move focus on, as Rename and Duplicate do; otherwise it goes back, not to the page.
    else requestAnimationFrame(() => (!document.activeElement || document.activeElement === document.body) && back?.focus());
  };

  const cardMenu = (p: LibraryProject): MenuEntry[] => {
    const sel = selection.includes(p.path) && selection.length > 1 ? selection : [p.path];
    const many = sel.length > 1;
    const reason = blocked(p);
    const isOpen = sel.includes(current?.path ?? "");
    const shared = (useLibrary.getState().projects ?? []).filter((x) => sel.includes(x.path)).map((x) => x.collection);
    const same = shared.every((c) => c === shared[0]) ? shared[0] : undefined;
    return [
      { label: "Open", icon: <FolderOpen size={15} />, shortcut: "Enter", run: () => open(p), disabled: many ? "Open one project at a time." : reason },
      { label: "Rename", icon: <Pencil size={15} />, shortcut: "F2", run: () => useLibrary.setState({ renaming: p.path }), disabled: many ? "Rename one project at a time." : reason },
      { label: "Duplicate", icon: <Copy size={15} />, shortcut: "Ctrl+D", run: () => void duplicateProject(p), disabled: many ? "Duplicate one project at a time." : p.state === "broken" ? reason : null },
      {
        label: "Move to collection",
        icon: <FolderInput size={15} />,
        disabled: p.state === "broken" ? reason : null,
        submenu: [
          { label: "No collection", checked: same === null, run: () => void moveToCollection(sel, null) },
          ...collections.map((c) => ({ label: c.name, checked: same === c.id, run: () => void moveToCollection(sel, c.id) })),
          "separator" as const,
          { label: "New collection…", icon: <FolderPlus size={15} />, run: () => setCreating(sel) },
        ],
      },
      { label: "Show in folder", icon: <Folder size={15} />, run: () => void revealItemInDir(p.path), disabled: many ? "Show one project at a time." : null },
      "separator",
      {
        label: many ? `Move ${sel.length} to Trash` : "Move to Trash",
        icon: <Trash2 size={15} />,
        shortcut: "Delete",
        danger: true,
        run: () => void trashProjects(sel),
        disabled: isOpen ? "This project is open. Open another project first, then move it to the Trash." : p.state === "busy" ? reason : null,
      },
    ];
  };

  const openCardMenu = (p: LibraryProject, at: { x: number; y: number }, keyboard: boolean) => {
    if (!selection.includes(p.path)) select([]);
    setFocusPath(p.path);
    showMenu(cardMenu(p), at, p.name, keyboard, p.path);
  };

  /** Dragging cards onto a collection in the sidebar moves them there. */
  const startDrag = (p: LibraryProject, e: PointerEvent<HTMLDivElement>) => {
    const start = { x: e.clientX, y: e.clientY };
    const paths = selection.includes(p.path) ? selection : [p.path];
    let dragging = false;
    const target = (x: number, y: number) => document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-drop]")?.dataset.drop ?? null;
    followPointer({
      move: (ev) => {
        if (!dragging && Math.hypot(ev.clientX - start.x, ev.clientY - start.y) > 5) dragging = p.state !== "broken";
        if (dragging) setDrag({ paths, x: ev.clientX, y: ev.clientY, over: target(ev.clientX, ev.clientY) });
      },
      up: (ev) => {
        if (!dragging) return;
        draggedAt.current = Date.now();
        setDrag(null);
        const over = target(ev.clientX, ev.clientY);
        if (over) void moveToCollection(paths, over);
      },
      cancel: () => setDrag(null),
    });
  };

  /** Arrow keys move between cards by where they sit, Enter opens, Space selects. */
  const onGridKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-path]");
    const p = el && found.find((x) => x.path === el.dataset.path);
    if (!el || !p) return;
    const mod = e.ctrlKey || e.metaKey;
    const go = (path: string | undefined) => {
      if (!path) return;
      e.preventDefault();
      setFocusPath(path);
      const card = cardEl(path);
      card?.focus();
      card?.scrollIntoView({ block: "nearest" });
    };
    const cards = [...(grid.current?.querySelectorAll<HTMLElement>("[data-path]") ?? [])];
    const index = cards.indexOf(el);
    const vertical = (dir: 1 | -1) => {
      const r = el.getBoundingClientRect();
      const rows = cards.filter((c) => (dir === 1 ? c.getBoundingClientRect().top > r.bottom : c.getBoundingClientRect().bottom < r.top));
      if (rows.length === 0) return undefined;
      const row = dir === 1 ? Math.min(...rows.map((c) => c.getBoundingClientRect().top)) : Math.max(...rows.map((c) => c.getBoundingClientRect().top));
      const centre = (c: HTMLElement) => c.getBoundingClientRect().left + c.getBoundingClientRect().width / 2;
      return rows
        .filter((c) => Math.abs(c.getBoundingClientRect().top - row) < 4)
        .sort((a, b) => Math.abs(centre(a) - centre(el)) - Math.abs(centre(b) - centre(el)))[0]?.dataset.path;
    };
    if (e.key === "ArrowRight") go(cards[index + 1]?.dataset.path);
    else if (e.key === "ArrowLeft") go(cards[index - 1]?.dataset.path);
    else if (e.key === "ArrowDown") go(vertical(1));
    else if (e.key === "ArrowUp") go(vertical(-1));
    else if (e.key === "Home") go(cards[0]?.dataset.path);
    else if (e.key === "End") go(cards[cards.length - 1]?.dataset.path);
    else if (e.key === "Enter") {
      e.preventDefault();
      open(p);
    } else if (e.key === " ") {
      e.preventDefault();
      toggle(p, e.shiftKey);
    } else if (e.key === "F2" && !blocked(p)) {
      e.preventDefault();
      useLibrary.setState({ renaming: p.path });
    } else if (e.key === "Delete" || e.key === "Backspace") {
      e.preventDefault();
      const paths = selection.length > 0 ? selection : [p.path];
      if (paths.includes(current?.path ?? "")) useEditor.getState().toast({ kind: "info", text: "The open project can't go to the Trash. Open another project first." });
      else void trashProjects(paths);
    } else if (mod && e.key.toLowerCase() === "a") {
      e.preventDefault();
      select(found.map((x) => x.path));
    } else if (mod && e.key.toLowerCase() === "d" && p.state !== "broken") {
      e.preventDefault();
      void duplicateProject(p);
    } else if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      e.preventDefault();
      const r = el.getBoundingClientRect();
      openCardMenu(p, { x: r.left + 12, y: r.top + 24 }, true);
    } else if (e.key === "Escape" && selection.length > 0) {
      e.preventDefault();
      e.stopPropagation();
      select([]);
    }
  };

  // Keys of the home screen: Esc goes back to the editor, typing searches.
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (menu || useLibrary.getState().pendingSwitch || document.querySelector("dialog[open]")) return;
      const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;
      const mod = e.ctrlKey || e.metaKey;
      const key = e.key.toLowerCase();
      if (e.key === "Escape" && !typing) {
        if (useLibrary.getState().selection.length > 0) select([]);
        else showEditor();
      } else if (mod && (key === "k" || key === "f")) {
        e.preventDefault();
        searchField.current?.focus();
        searchField.current?.select();
      } else if (mod && key === "z" && !e.shiftKey && !typing) {
        // Undo here takes back the last Trash, move or deleted collection, while its toast still offers it.
        if (undoLastToast()) e.preventDefault();
      } else if (mod && key === "j") {
        e.preventDefault();
        openAi();
      } else if (mod && key === "o") {
        e.preventDefault();
        void openProjectFile();
      } else if (mod && key === "n") {
        e.preventDefault();
        const r = document.querySelector("[data-new-project]")?.getBoundingClientRect();
        if (r) showMenu(newProjectItems(), { x: r.right, y: r.bottom + 6, align: "end" }, "New project", true);
      } else if (!typing && !mod && !e.altKey && e.key.length === 1 && e.key !== " ") {
        // Typing anywhere starts a search, as in the launcher.
        e.preventDefault();
        searchField.current?.focus();
        setQuery((q) => q + e.key);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const title = filter === "all" ? "All projects" : (collectionOf(filter)?.name ?? "All projects");
  const sections = useMemo(() => {
    // A few projects need no groups: each card says when it was opened.
    if (searching || sort === "name" || found.length < 12) return [{ title: null as string | null, items: found }];
    const groups: { title: string | null; items: LibraryProject[] }[] = [];
    for (const p of found) {
      const period = periodOf(p.modifiedMs);
      const last = groups[groups.length - 1];
      if (last?.title === period) last.items.push(p);
      else groups.push({ title: period, items: [p] });
    }
    return groups;
  }, [found, searching, sort]);

  return (
    <div className="flex h-full flex-col bg-bg" aria-label="Projects">
      <header className="flex h-12 shrink-0 items-center gap-2 px-1.5">
        <Button variant="bar" pill title="Back to the editor (Esc)" onClick={() => showEditor()} className="max-w-[340px] min-w-0">
          <ChevronLeft size={15} className="shrink-0" />
          <span className="truncate">{current?.project.name ?? "Editor"}</span>
        </Button>
        <div className="flex-1" />
        <AiRunBar />
        <JobIndicator />
        <AiButton />
      </header>
      <div className="flex min-h-0 flex-1 gap-1.5 px-1.5 pb-1.5">
        <Sidebar drag={drag} creating={creating} setCreating={setCreating} showMenu={showMenu} />
        <main className="pane relative flex min-w-0 flex-1 flex-col" aria-label={title}>
          <div className="flex h-16 shrink-0 items-center gap-2 pl-6 pr-4">
            <h1 className="max-w-[40%] shrink-0 truncate text-[17px] font-semibold text-fg">{title}</h1>
            {projects && !firstRun && <span className="tabular shrink-0 text-[13px] text-muted">{inView.length}</span>}
            <div className="min-w-4 flex-1" />
            {!firstRun && (
              <SearchField ref={searchField} value={query} onChange={setQuery} onEnter={focusFirstResult} onDown={focusFirstResult} />
            )}
            {!firstRun && (
            <Button
              variant="ghost"
              aria-haspopup="menu"
              title="Sort projects"
              onClick={(e) => {
                const r = e.currentTarget.getBoundingClientRect();
                showMenu(
                  [
                    { label: "Recent", checked: sort === "opened", run: () => setSort("opened") },
                    { label: "Name", checked: sort === "name", run: () => setSort("name") },
                  ],
                  { x: r.right, y: r.bottom + 6, align: "end" },
                  "Sort by",
                  e.detail === 0,
                );
              }}
              className="shrink-0"
            >
              <ArrowUpDown size={14} /> {sort === "name" ? "Name" : "Recent"} <ChevronDown size={13} />
            </Button>
            )}
            <IconButton label="Open file… (Ctrl+O)" onClick={() => void openProjectFile()}>
              <FolderOpen size={16} />
            </IconButton>
            <Button
              variant="primary"
              data-new-project
              aria-haspopup="menu"
              title="New project (Ctrl+N)"
              onClick={(e) => {
                const r = e.currentTarget.getBoundingClientRect();
                showMenu(newProjectItems(), { x: r.right, y: r.bottom + 6, align: "end" }, "New project", e.detail === 0);
              }}
              className="shrink-0"
            >
              <Plus size={15} /> New project <ChevronDown size={13} />
            </Button>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-24">
            {firstRun ? (
              <FirstRun />
            ) : projects === null ? (
              loadError ? (
                <Empty icon={<AlertCircle size={28} className="text-danger" />} title="Your projects couldn't be listed">
                  <span className="block">{loadError}</span>
                  <Button pill onClick={() => void refreshLibrary()} className="mt-4">
                    Try again
                  </Button>
                </Empty>
              ) : (
                <Skeleton />
              )
            ) : (
              <>
                {found.length > 0 && (
                  <div ref={grid} role="listbox" aria-label="Projects" aria-multiselectable onKeyDown={onGridKey} className="flex flex-col gap-7 pt-1">
                    {sections.map((section) => (
                      <section key={section.title ?? "all"} role="group" aria-label={section.title ?? undefined}>
                        {(section.title ?? (searching ? "Projects" : null)) && <h2 className="mb-3 text-[13px] font-semibold text-fg">{section.title ?? "Projects"}</h2>}
                        <div className="grid grid-cols-[repeat(auto-fill,minmax(148px,168px))] gap-x-4 gap-y-5">
                          {section.items.map((p) => (
                            <ProjectCard
                              key={p.path}
                              p={p}
                              focusable={p.path === tabStop}
                              selected={selection.includes(p.path)}
                              menuOpen={menu?.target === p.path}
                              selecting={selection.length > 0}
                              onOpen={() => open(p)}
                              onToggle={(e) => toggle(p, e.shiftKey)}
                              onMenu={(at, keyboard) => openCardMenu(p, at, keyboard)}
                              onDragStart={(e) => startDrag(p, e)}
                            />
                          ))}
                        </div>
                      </section>
                    ))}
                  </div>
                )}
                {searching && <SaidResults query={query} said={said} hits={hits} projects={projects} first={found.length === 0} />}
                {found.length === 0 && !searching && filter !== "all" && (
                  <Empty icon={<Folder size={28} className="text-muted" />} title={`No projects in ${title} yet`}>
                    <span className="block">Drag projects onto it from All projects, or start a new one here.</span>
                    <Button pill onClick={() => useLibrary.setState({ filter: "all" })} className="mt-4">
                      Show all projects
                    </Button>
                  </Empty>
                )}
                {searching && found.length === 0 && hits.length === 0 && !said.searching && said.query === query && (
                  <div className="flex flex-col items-center gap-2 pt-16 text-[13px] text-muted">
                    <span>Nothing matches “{query.trim()}”{filter === "all" ? "" : ` in ${title}`}.</span>
                    <Button variant="ghost" onClick={() => setQuery("")} className="text-accent! hover:text-accent!">
                      Clear search
                    </Button>
                  </div>
                )}
              </>
            )}
          </div>
          {selection.length > 0 && <SelectionBar showMenu={showMenu} newCollection={setCreating} />}
          {fileDrag && (
            <div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center rounded-[12px] bg-black/45">
              <div className="overlay flex w-[320px] flex-col items-center gap-2 rounded-[20px] px-6 py-7 text-center">
                <Upload size={28} className="text-accent" />
                <div className="text-[15px] font-semibold text-fg">Drop to start a new project</div>
                <div className="text-[13px] text-muted">It takes the format of the first video.</div>
              </div>
            </div>
          )}
        </main>
      </div>
      {menu && <Menu items={menu.items} at={menu.at} label={menu.label} keyboard={menu.keyboard} onClose={closeMenu} />}
      {drag && (
        <div className="pointer-events-none fixed z-[120] rounded-lg bg-raised px-2.5 py-1 text-[12px] font-medium text-fg shadow-lg shadow-black/50 ring-1 ring-white/10" style={{ left: drag.x + 12, top: drag.y + 10 }}>
          {drag.paths.length === 1 ? (projects?.find((p) => p.path === drag.paths[0])?.name ?? "Project") : `${drag.paths.length} projects`}
        </div>
      )}
    </div>
  );
}

function SearchField({ ref, value, onChange, onEnter, onDown }: { ref: Ref<HTMLInputElement>; value: string; onChange: (v: string) => void; onEnter: () => void; onDown: () => void }) {
  return (
    <label className="relative flex h-8 w-[300px] min-w-[180px] shrink items-center">
      <Search size={14} className="pointer-events-none absolute left-2.5 text-muted" />
      <input
        ref={ref}
        type="search"
        value={value}
        placeholder="Search names and what was said"
        aria-label="Search projects and what was said in them"
        title="Search (Ctrl+K)"
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && value) {
            e.stopPropagation();
            onChange("");
          } else if (e.key === "Enter") onEnter();
          else if (e.key === "ArrowDown") {
            e.preventDefault();
            onDown();
          }
        }}
        className={`h-8 w-full text-ellipsis rounded-lg border border-white/[.08] bg-white/[.055] pl-8 text-[13px] text-fg outline-none placeholder:text-muted focus:border-accent [&::-webkit-search-cancel-button]:hidden ${value ? "pr-7" : "pr-2"}`}
      />
      {value && (
        <button type="button" aria-label="Clear search" title="Clear search" onClick={() => onChange("")} className="absolute right-1.5 flex h-5 w-5 items-center justify-center rounded-full text-muted hover:bg-white/[.08] hover:text-fg">
          <X size={12} />
        </button>
      )}
    </label>
  );
}

/**
 * What the search through speech could not cover: nothing transcribed yet, or which projects were
 * searched by name only. `inScope` limits the projects named to those shown.
 */
export function SpeechNote({ query, said, hits, projects, inScope }: { query: string; said: SaidState; hits: number; projects: LibraryProject[]; inScope: (p: LibraryProject) => boolean }) {
  const result = said.result && said.query === query ? said.result : null;
  if (!result || said.searching) return null;
  const untranscribed = result.untranscribed.map((path) => projects.find((p) => p.path === path)).filter((p): p is LibraryProject => !!p && inScope(p));
  const note = (text: string) => (
    <p className="flex items-start gap-1.5 py-1 text-[12px] text-muted">
      <Info size={13} className="mt-px shrink-0" />
      {text}
    </p>
  );
  if (result.transcribed === 0) return note("None of your projects is transcribed yet, so only names were searched. Transcribe a project in its Transcript tab.");
  return (
    <>
      {hits === 0 && <p className="py-1 text-[13px] text-muted">Nobody says “{query.trim()}” in your transcribed videos.</p>}
      {untranscribed.length > 0 &&
        note(
          untranscribed.length <= 2
            ? `${untranscribed.map((p) => p.name).join(" and ")} ${untranscribed.length === 1 ? "isn't" : "aren't"} transcribed, so only ${untranscribed.length === 1 ? "its name was" : "their names were"} searched.`
            : `${untranscribed.length} projects aren't transcribed, so only their names were searched.`,
        )}
    </>
  );
}

type SaidState = { query: string; result: Said | null; searching: boolean };

/** Quotes of where the words were said, each opening its project at that moment. */
function SaidResults({ query, said, hits, projects, first }: { query: string; said: SaidState; hits: Said["hits"]; projects: LibraryProject[]; /** No project names above it. */ first: boolean }) {
  const filter = useLibrary((s) => s.filter);
  if (fold(query).replace(/\s+/g, "").length < 2) return null;
  return (
    <section aria-label="Said in your videos" className={first ? "pt-1" : "mt-8"}>
      <h2 className="mb-2 flex items-center gap-2 text-[13px] font-semibold text-fg">
        Said in your videos
        {said.searching && <Loader2 size={13} className="animate-spin text-muted" aria-label="Searching" />}
      </h2>
      <div className="flex flex-col gap-0.5">
        {hits.map((hit) => {
          const p = projects.find((x) => x.path === hit.path);
          if (!p) return null;
          return (
            <button
              key={`${hit.path}:${hit.startUs}`}
              type="button"
              onClick={() => openProject(p, hit.startUs)}
              title={`Open ${p.name} at ${formatLength(hit.startUs)}`}
              className="-mx-2 flex items-center gap-3 rounded-lg px-2 py-1.5 text-left transition-colors duration-[120ms] hover:bg-white/[.06]"
            >
              <span className="relative h-10 w-10 shrink-0 overflow-hidden rounded-md bg-stage">
                <Poster p={p} box={1} compact />
              </span>
              <span className="min-w-0 flex-1">
                <span className="flex items-baseline gap-2">
                  <span className="truncate text-[13px] font-medium text-fg">{p.name}</span>
                  <span className="tabular shrink-0 text-[12px] text-muted">{formatLength(hit.startUs)}</span>
                </span>
                <Quote hit={hit} />
              </span>
            </button>
          );
        })}
      </div>
      <SpeechNote query={query} said={said} hits={hits.length} projects={projects} inScope={(p) => filter === "all" || p.collection === filter} />
    </section>
  );
}

export function Quote({ hit }: { hit: Said["hits"][number] }) {
  return (
    <span className="block truncate text-[12px] text-muted">
      “{hit.before && "…"}
      {hit.before} <mark className="rounded-[3px] bg-accent/25 px-0.5 text-fg">{hit.text}</mark> {hit.after}
      {hit.after && "…"}”
    </span>
  );
}

/** The shapes of cards while the list loads, so nothing jumps when it arrives. */
function Skeleton() {
  return (
    <div aria-busy aria-label="Loading projects" className="grid grid-cols-[repeat(auto-fill,minmax(148px,168px))] gap-x-4 gap-y-5 pt-1">
      {Array.from({ length: 6 }, (_, i) => (
        <div key={i}>
          <div className="w-full rounded-[10px] bg-white/[.04]" style={{ aspectRatio: `${152 / 180}` }} />
          <div className="mt-2.5 h-3 w-3/5 rounded bg-white/[.05]" />
          <div className="mt-2 h-2.5 w-2/5 rounded bg-white/[.04]" />
        </div>
      ))}
    </div>
  );
}

function Empty({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-2 pt-24 text-center">
      {icon}
      <div className="text-[15px] font-semibold text-fg">{title}</div>
      <div className="max-w-[360px] text-[13px] text-muted">{children}</div>
    </div>
  );
}

/** No projects yet: the formats to start with, and that dropping videos works too. */
function FirstRun() {
  return (
    <div className="flex flex-col items-center gap-6 pt-20">
      <h2 className="text-[17px] font-semibold text-fg">Start your first project</h2>
      <div className="flex flex-wrap justify-center gap-3">
        {FORMATS.map((f) => (
          <button
            key={f.label}
            type="button"
            onClick={() => newProject(f.width, f.height)}
            className="flex w-[140px] flex-col items-center gap-2 rounded-xl bg-white/[.05] px-3 pt-5 pb-4 text-fg/80 outline outline-1 -outline-offset-1 outline-white/[.07] transition-colors duration-[120ms] ease-out hover:bg-white/[.09] hover:text-fg"
          >
            <span className="flex h-12 items-center">
              <FormatShape width={f.width} height={f.height} size={44} />
            </span>
            <span className="tabular text-[13px] font-semibold text-fg">{f.label}</span>
            <span className="text-[11px] text-muted">{f.hint}</span>
          </button>
        ))}
        <button
          type="button"
          onClick={() => void pickVideosForNewProject()}
          className="flex w-[140px] flex-col items-center gap-2 rounded-xl bg-white/[.05] px-3 pt-5 pb-4 outline outline-1 -outline-offset-1 outline-white/[.07] transition-colors duration-[120ms] ease-out hover:bg-white/[.09]"
        >
          <span className="flex h-12 items-center text-muted">
            <Film size={24} />
          </span>
          <span className="text-[13px] font-semibold text-fg">From videos…</span>
          <span className="text-[11px] text-muted">First video's format</span>
        </button>
      </div>
      <p className="text-[13px] text-muted">Or drop videos from your phone anywhere in this window.</p>
    </div>
  );
}

/** What can be done with the selected projects at once. */
function SelectionBar({ showMenu, newCollection }: { showMenu: (items: MenuEntry[], at: MenuState["at"], label: string, keyboard: boolean) => void; newCollection: (paths: string[]) => void }) {
  const selection = useLibrary((s) => s.selection);
  const collections = useLibrary((s) => s.collections);
  const current = useEditor((s) => s.snap?.path);
  const hasOpen = selection.includes(current ?? "");
  return (
    <div role="toolbar" aria-label="Selected projects" className="overlay absolute bottom-4 left-1/2 z-10 flex h-11 -translate-x-1/2 items-center gap-1 rounded-full pl-4 pr-1.5 text-[13px] text-fg">
      <span className="tabular pr-2 font-medium">{selection.length} selected</span>
      <span className="h-5 w-px bg-white/[.12]" />
      <Button
        variant="ghost"
        pill
        aria-haspopup="menu"
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          showMenu(
            [
              { label: "No collection", run: () => void moveToCollection(selection, null) },
              ...collections.map((c) => ({ label: c.name, run: () => void moveToCollection(selection, c.id) })),
              "separator" as const,
              { label: "New collection…", icon: <FolderPlus size={15} />, run: () => newCollection(selection) },
            ],
            { x: r.left, y: r.top - 6, above: true },
            "Move to collection",
            e.detail === 0,
          );
        }}
      >
        <FolderInput size={14} /> Move to collection <ChevronDown size={13} />
      </Button>
      <Button
        variant="ghost"
        pill
        disabled={hasOpen}
        disabledReason="The open project can't go to the Trash. Open another project first."
        onClick={() => void trashProjects(selection)}
        className="text-danger! hover:text-danger!"
      >
        <Trash2 size={14} /> Move to Trash
      </Button>
      <span className="h-5 w-px bg-white/[.12]" />
      <IconButton round label="Clear selection (Esc)" onClick={() => useLibrary.setState({ selection: [] })}>
        <X size={15} />
      </IconButton>
    </div>
  );
}

/** All projects, then the collections; projects dragged onto a collection move there. */
function Sidebar({
  drag,
  creating,
  setCreating,
  showMenu,
}: {
  drag: { over: string | null } | null;
  /** Projects waiting for a new collection, or null; an empty list is a new collection from the sidebar. */
  creating: string[] | null;
  setCreating: (paths: string[] | null) => void;
  showMenu: (items: MenuEntry[], at: MenuState["at"], label: string, keyboard: boolean) => void;
}) {
  const projects = useLibrary((s) => s.projects);
  const collections = useLibrary((s) => s.collections);
  const filter = useLibrary((s) => s.filter);
  const empty = onlyPristine(projects, useEditor((s) => s.snap));
  const [renaming, setRenaming] = useState<string | null>(null);
  const count = (id: string) => projects?.filter((p) => p.collection === id).length ?? 0;
  const pick = (id: string) => useLibrary.setState({ filter: id, selection: [] });

  const rowKeys = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const rows = [...(e.currentTarget.closest("nav")?.querySelectorAll<HTMLElement>("[data-row]") ?? [])];
    rows[rows.indexOf(e.currentTarget) + (e.key === "ArrowDown" ? 1 : -1)]?.focus();
  };
  const collectionMenu = (c: Collection, at: { x: number; y: number }, keyboard: boolean) =>
    showMenu(
      [
        { label: "Rename", icon: <Pencil size={15} />, shortcut: "F2", run: () => setRenaming(c.id) },
        { label: "Delete collection", icon: <Trash2 size={15} />, danger: true, run: () => void deleteCollection(c.id) },
      ],
      at,
      c.name,
      keyboard,
    );

  return (
    <div className="pane flex w-[232px] shrink-0 flex-col">
      <nav aria-label="Collections" className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto p-2">
        <Row icon={<LayoutGrid size={16} />} label="All projects" count={empty ? 0 : projects?.length} on={filter === "all"} onClick={() => pick("all")} onKeyDown={rowKeys} />
        <div className="mt-4 mb-1 flex h-7 items-center justify-between pl-2.5">
          <h2 className="text-[13px] font-semibold text-fg">Collections</h2>
          <IconButton label="New collection" onClick={() => setCreating([])} className="h-7 w-7">
            <Plus size={15} />
          </IconButton>
        </div>
        {collections.map((c) =>
          renaming === c.id ? (
            <NameField
              key={c.id}
              initial={c.name}
              label="Collection name"
              onDone={(name) => {
                setRenaming(null);
                if (name !== null) void renameCollection(c.id, name);
              }}
            />
          ) : (
            <Row
              key={c.id}
              drop={c.id}
              dropping={!!drag}
              over={drag?.over === c.id}
              icon={<Folder size={16} />}
              label={c.name}
              count={count(c.id)}
              on={filter === c.id}
              onClick={() => pick(c.id)}
              onKeyDown={(e) => {
                if (e.key === "F2") {
                  e.preventDefault();
                  setRenaming(c.id);
                } else if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
                  e.preventDefault();
                  const r = e.currentTarget.getBoundingClientRect();
                  collectionMenu(c, { x: r.left + 12, y: r.bottom }, true);
                } else rowKeys(e);
              }}
              onContextMenu={(at) => collectionMenu(c, at, false)}
            />
          ),
        )}
        {creating !== null ? (
          <NameField
            initial=""
            label="New collection name"
            onDone={async (name) => {
              const paths = creating;
              setCreating(null);
              if (name === null) return;
              const created = await createCollection(name, paths);
              if (!created) return;
              if (paths.length === 0) pick(created.id);
              else useEditor.getState().toast({ kind: "success", text: paths.length === 1 ? `Moved the project to ${created.name}.` : `Moved ${paths.length} projects to ${created.name}.` });
            }}
          />
        ) : (
          collections.length === 0 && (
            <button
              type="button"
              data-row
              onKeyDown={rowKeys}
              onClick={() => setCreating([])}
              className="flex h-8 items-center gap-2.5 rounded-lg px-2.5 text-left text-[13px] text-muted transition-colors duration-[120ms] hover:bg-white/[.05] hover:text-fg"
            >
              <FolderPlus size={16} /> New collection
            </button>
          )
        )}
      </nav>
      <div className="shrink-0 p-2 pt-0">
        <VersionRow showMenu={showMenu} />
      </div>
    </div>
  );
}

function Row({
  icon,
  label,
  count,
  on,
  onClick,
  onKeyDown,
  onContextMenu,
  drop,
  dropping = false,
  over = false,
}: {
  icon: ReactNode;
  label: string;
  count: number | undefined;
  on: boolean;
  onClick: () => void;
  onKeyDown: (e: KeyboardEvent<HTMLButtonElement>) => void;
  onContextMenu?: (at: { x: number; y: number }) => void;
  drop?: string;
  dropping?: boolean;
  over?: boolean;
}) {
  return (
    <button
      type="button"
      data-row
      data-drop={drop}
      aria-current={on ? "page" : undefined}
      onClick={onClick}
      onKeyDown={onKeyDown}
      onContextMenu={
        onContextMenu &&
        ((e) => {
          e.preventDefault();
          onContextMenu({ x: e.clientX, y: e.clientY });
        })
      }
      className={`flex h-8 shrink-0 items-center gap-2.5 rounded-lg px-2.5 text-left text-[13px] transition-colors duration-[120ms] ease-out ${
        over ? "bg-accent/20 text-fg outline outline-1 -outline-offset-1 outline-accent" : on ? "bg-white/[.08] font-medium text-fg" : dropping ? "text-fg hover:bg-white/[.05]" : "text-fg/90 hover:bg-white/[.05]"
      }`}
    >
      <span className={`shrink-0 ${on || over ? "text-accent" : "text-muted"}`}>{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {count !== undefined && <span className="tabular shrink-0 text-[12px] text-muted">{count}</span>}
    </button>
  );
}

/** A name typed in place: Enter saves, Esc or an empty name cancels. */
function NameField({ initial, label, onDone }: { initial: string; label: string; onDone: (name: string | null) => void }) {
  const [draft, setDraft] = useState(initial);
  const done = useRef(false);
  const finish = (save: boolean) => {
    if (done.current) return;
    done.current = true;
    onDone(save && draft.trim() ? draft.trim() : null);
  };
  return (
    <div className="flex h-8 shrink-0 items-center gap-2.5 rounded-lg bg-white/[.05] px-2.5">
      <Folder size={16} className="shrink-0 text-accent" />
      <input
        autoFocus
        aria-label={label}
        placeholder="Collection name"
        value={draft}
        maxLength={60}
        onFocus={(e) => e.currentTarget.select()}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => finish(true)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") finish(true);
          if (e.key === "Escape") finish(false);
        }}
        className="min-w-0 flex-1 bg-transparent text-[13px] text-fg outline-none placeholder:text-muted"
      />
    </div>
  );
}

