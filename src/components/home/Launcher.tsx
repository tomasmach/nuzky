import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { ChevronRight, CornerDownLeft, FolderOpen, LayoutGrid, Loader2, Lock, Search, Sparkles, TriangleAlert, Upload, X } from "lucide-react";
import {
  blocked,
  fold,
  matches,
  newProject,
  openProject,
  openProjectFile,
  pickVideosForNewProject,
  refreshLibrary,
  resolveSwitch,
  showEditor,
  showHome,
  useLibrary,
  useSaidSearch,
  whenLabel,
} from "../../lib/library";
import { FORMATS, formatLabel } from "../../lib/presets";
import { useEditor } from "../../lib/store";
import { formatLength } from "../../lib/time";
import type { LibraryProject, SaidHit } from "../../lib/types";
import { Button, trapTab } from "../ui";
import { FormatShape, Quote, SpeechNote } from "./Home";
import { Poster } from "./Poster";
import { SwitchChoice } from "./SwitchConfirm";

const RECENT = 5;
const FOUND = 6;
const SAID = 6;

type Option = { id: string; run: () => void; node: (active: boolean) => ReactNode; disabled?: string | null };

function Thumb({ p }: { p: LibraryProject }) {
  return (
    <span className="relative h-9 w-9 shrink-0 overflow-hidden rounded-md bg-stage outline outline-1 -outline-offset-1 outline-white/[.06]">
      <Poster p={p} box={1} compact />
    </span>
  );
}

/** What a row says about a project: where it is and its shape, or why it cannot open. */
function detail(p: LibraryProject, collection: string | undefined): ReactNode {
  if (p.state === "busy")
    return (
      <>
        <Lock size={12} className="shrink-0" /> In use elsewhere
      </>
    );
  if (p.state === "broken") return <span className="text-danger">Damaged file</span>;
  if (p.state === "missing")
    return (
      <>
        <TriangleAlert size={12} className="shrink-0 text-warn" /> {p.missing} {p.missing === 1 ? "file" : "files"} missing
      </>
    );
  const parts = [collection, formatLabel(p.width, p.height), p.state === "empty" ? "Empty" : formatLength(p.durationUs)].filter(Boolean);
  return <span className="tabular">{parts.join(" · ")}</span>;
}

function Row({ active, thumb, title, sub, end }: { active: boolean; thumb: ReactNode; title: ReactNode; sub: ReactNode; end?: ReactNode }) {
  return (
    <div className={`flex h-12 items-center gap-3 rounded-[10px] px-2 ${active ? "bg-white/[.08]" : ""}`}>
      {thumb}
      <div className="min-w-0 flex-1">
        <div className="truncate text-[13px] font-medium text-fg">{title}</div>
        <div className="flex min-w-0 items-center gap-1 truncate text-[12px] text-muted">{sub}</div>
      </div>
      {end && <div className="flex shrink-0 items-center gap-2 text-[12px] text-muted">{end}</div>}
      {active && (
        <span aria-hidden className="flex h-5 w-6 shrink-0 items-center justify-center rounded-[5px] border border-white/15 text-muted">
          <CornerDownLeft size={11} />
        </span>
      )}
    </div>
  );
}

/**
 * Over the editor (Projects, Ctrl+K): the open project, the recent ones, new projects in each format,
 * and search through names and what was said. The keyboard stays in the search field: ↑/↓ move,
 * Enter opens, Esc goes back to editing.
 */
export function Launcher() {
  const projects = useLibrary((s) => s.projects);
  const collections = useLibrary((s) => s.collections);
  const pending = useLibrary((s) => s.pendingSwitch);
  const snap = useEditor((s) => s.snap);
  const aiRun = useEditor((s) => s.aiRun);
  const [query, setQuery] = useState("");
  const said = useSaidSearch(query);
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);

  useEffect(() => {
    void refreshLibrary();
    const back = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => back?.focus();
  }, []);
  // Back in the field once the question about the agent's changes is answered.
  useEffect(() => {
    if (!pending) input.current?.focus();
  }, [pending]);

  const close = () => (pending ? void resolveSwitch("cancel") : useEditor.setState({ launcherOpen: false }));
  const collectionName = (p: LibraryProject) => collections.find((c) => c.id === p.collection)?.name;
  const searching = query.trim().length > 0;
  const currentProject = projects?.find((p) => p.path === snap?.path);
  const recent = useMemo(() => (projects ?? []).filter((p) => p.path !== snap?.path).slice(0, RECENT), [projects, snap?.path]);
  const found = useMemo(() => (searching ? (projects ?? []).filter((p) => matches(p, query, collections)).slice(0, FOUND) : []), [projects, query, collections, searching]);
  const hits = said.result && said.query === query ? said.result.hits.slice(0, SAID) : [];

  const projectOption = (p: LibraryProject, end?: ReactNode): Option => ({
    id: `p:${p.path}`,
    disabled: blocked(p),
    run: () => openProject(p),
    node: (on) => <Row active={on} thumb={<Thumb p={p} />} title={p.name} sub={detail(p, collectionName(p))} end={end ?? whenLabel(p.modifiedMs)} />,
  });
  const hitOption = (hit: SaidHit): Option | null => {
    const p = projects?.find((x) => x.path === hit.path);
    if (!p) return null;
    return {
      id: `s:${hit.path}:${hit.startUs}`,
      disabled: blocked(p),
      run: () => openProject(p, hit.startUs),
      node: (on) => (
        <Row
          active={on}
          thumb={<Thumb p={p} />}
          title={
            <>
              {p.name} <span className="tabular pl-1 font-normal text-muted">{formatLength(hit.startUs)}</span>
            </>
          }
          sub={<Quote hit={hit} />}
        />
      ),
    };
  };
  const allOption: Option = {
    id: "all",
    run: showHome,
    node: (on) => (
      <div className={`flex h-10 items-center gap-3 rounded-[10px] px-2 text-[13px] ${on ? "bg-white/[.08]" : ""}`}>
        <span className="flex w-9 justify-center text-muted">
          <LayoutGrid size={16} />
        </span>
        <span className="flex-1 font-medium text-fg">All projects</span>
        <span className="tabular text-[12px] text-muted">{projects?.length ?? ""}</span>
        <ChevronRight size={14} className="text-muted" />
      </div>
    ),
  };

  const allFound = useMemo(() => (searching ? (projects ?? []).filter((p) => matches(p, query, collections)).length : 0), [projects, query, collections, searching]);
  const allHits = said.result && said.query === query ? said.result.hits.length : 0;
  // More than fit here: the home screen shows all of them, with the same search.
  const moreOption: Option = {
    id: "more",
    run: () => {
      useLibrary.setState({ handoffQuery: query, filter: "all" });
      showHome();
    },
    node: (on) => (
      <div className={`flex h-10 items-center gap-3 rounded-[10px] px-2 text-[13px] ${on ? "bg-white/[.08]" : ""}`}>
        <span className="flex w-9 justify-center text-muted">
          <LayoutGrid size={16} />
        </span>
        <span className="flex-1 font-medium text-fg">Show all results</span>
        <ChevronRight size={14} className="text-muted" />
      </div>
    ),
  };
  const groups: { title: string; options: Option[] }[] = searching
    ? [
        { title: "Projects", options: found.map((p) => projectOption(p)) },
        {
          title: "Said in your videos",
          options: [...hits.map(hitOption).filter((o): o is Option => !!o), ...(allFound > FOUND || allHits > SAID ? [moreOption] : [])],
        },
      ]
    : [
        ...(currentProject
          ? [
              {
                title: "Continue",
                options: [
                  {
                    id: "continue",
                    run: () => showEditor(),
                    node: (on: boolean) => (
                      <Row
                        active={on}
                        thumb={<Thumb p={currentProject} />}
                        title={currentProject.name}
                        sub={
                          aiRun ? (
                            <>
                              <Sparkles size={12} className="shrink-0 text-accent" /> AI is editing · {aiRun}
                            </>
                          ) : (
                            detail(currentProject, collectionName(currentProject))
                          )
                        }
                        end="Open now"
                      />
                    ),
                  },
                ],
              },
            ]
          : []),
        { title: "Recent", options: [...recent.map((p) => projectOption(p)), allOption] },
      ];
  const options = groups.flatMap((g) => g.options);
  const at = Math.min(active, Math.max(0, options.length - 1));

  // The first result is where Enter goes, as in Spotlight.
  useEffect(() => setActive(0), [query]);
  useEffect(() => {
    list.current?.querySelector(`[data-option="${at}"]`)?.scrollIntoView({ block: "nearest" });
  }, [at]);

  const choose = (option: Option | undefined) => {
    if (!option) return;
    if (option.disabled) return useEditor.getState().toast({ kind: "info", text: option.disabled });
    option.run();
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    e.stopPropagation();
    trapTab(e);
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (pending) {
      return;
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (options.length > 0) setActive((at + (e.key === "ArrowDown" ? 1 : -1) + options.length) % options.length);
    } else if (e.key === "Enter" && e.target === input.current) {
      e.preventDefault();
      choose(options[at]);
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      close();
    }
  };

  let index = 0;
  return (
    <div className="fixed inset-0 z-[80]" onKeyDown={onKeyDown}>
      <div className="scrim-in absolute inset-0 bg-black/45" onPointerDown={close} />
      <div className="pointer-events-none absolute inset-x-0 top-[88px] flex justify-center px-6">
        <div
          role="dialog"
          aria-modal
          aria-label="Projects"
          className="dialog-in overlay overlay-thick pointer-events-auto flex max-h-[calc(100vh-112px)] w-[680px] max-w-full flex-col rounded-[20px] p-2.5"
        >
          <label className="flex h-12 shrink-0 items-center gap-3 rounded-[14px] border border-white/[.08] bg-white/[.06] px-3.5 focus-within:border-accent">
            <Search size={18} className="shrink-0 text-muted" />
            <input
              ref={input}
              role="combobox"
              aria-expanded
              aria-controls="launcher-list"
              aria-activedescendant={options.length > 0 ? `launcher-option-${at}` : undefined}
              aria-label="Search projects and what was said in them"
              placeholder="Search projects and what was said"
              value={query}
              disabled={!!pending}
              onChange={(e) => setQuery(e.target.value)}
              className="min-w-0 flex-1 bg-transparent text-[17px] text-fg outline-none placeholder:text-muted disabled:opacity-60"
            />
            {query && (
              <button type="button" aria-label="Clear search" title="Clear search" onClick={() => (setQuery(""), input.current?.focus())} className="flex h-6 w-6 items-center justify-center rounded-full text-muted hover:bg-white/[.08] hover:text-fg">
                <X size={14} />
              </button>
            )}
          </label>
          <div ref={list} id="launcher-list" role="listbox" aria-label="Projects" className="mt-2 min-h-0 flex-1 overflow-y-auto">
            {groups.map((group) => (
              <div key={group.title} role="group" aria-label={group.title} className="pb-1">
                {(group.options.length > 0 || searching) && (
                  <div className="flex items-center gap-2 px-2 pt-2 pb-1 text-[12px] font-semibold text-muted">
                    {group.title}
                    {group.title === "Said in your videos" && said.searching && <Loader2 size={12} className="animate-spin" aria-label="Searching" />}
                  </div>
                )}
                {group.options.map((option) => {
                  const i = index++;
                  return (
                    <div
                      key={option.id}
                      id={`launcher-option-${i}`}
                      data-option={i}
                      role="option"
                      // While the question about an AI run is open, Enter answers it, so no row looks chosen.
                      aria-selected={i === at && !pending}
                      aria-disabled={option.disabled ? true : undefined}
                      title={option.disabled ?? undefined}
                      onPointerMove={() => i !== at && setActive(i)}
                      onClick={() => choose(option)}
                      className={option.disabled ? "opacity-60" : ""}
                    >
                      {option.node(i === at && !pending)}
                    </div>
                  );
                })}
                {searching && group.title === "Projects" && group.options.length === 0 && <div className="px-2 py-1.5 text-[13px] text-muted">No project names match.</div>}
                {searching && group.title !== "Projects" && (
                  <div className="px-2">
                    {fold(query).replace(/\s+/g, "").length < 2 ? (
                      <p className="py-1 text-[13px] text-muted">Type a little more to search what was said.</p>
                    ) : (
                      <SpeechNote query={query} said={said} hits={hits.length} projects={projects ?? []} inScope={() => true} />
                    )}
                  </div>
                )}
              </div>
            ))}
          </div>
          {pending ? (
            <div className="mt-1 shrink-0 rounded-[14px] bg-white/[.05] p-4 outline outline-1 -outline-offset-1 outline-white/[.08]">
              <SwitchChoice pending={pending} />
            </div>
          ) : (
            <>
              <div className="mt-1 flex shrink-0 items-center gap-1.5 border-t border-white/[.08] px-1 pt-2.5">
                <span className="pr-1 pl-1 text-[12px] text-muted">New</span>
                {FORMATS.map((f) => (
                  <Button key={f.label} pill title={`New ${f.label} project · ${f.hint}`} onClick={() => newProject(f.width, f.height)} className="tabular h-7 gap-1.5 px-2.5 text-[12px]">
                    <FormatShape width={f.width} height={f.height} size={11} />
                    {f.label}
                  </Button>
                ))}
                <Button variant="ghost" pill onClick={() => void pickVideosForNewProject()} className="h-7 px-2.5 text-[12px]">
                  <Upload size={13} /> From videos…
                </Button>
                <div className="flex-1" />
                <Button variant="ghost" pill onClick={() => void openProjectFile()} className="h-7 px-2.5 text-[12px]">
                  <FolderOpen size={13} /> Open file…
                </Button>
              </div>
              <div className="flex shrink-0 justify-end px-2 pt-2 pb-0.5 text-[11px] text-muted">↑↓ move · ↵ open · Esc keep editing</div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
