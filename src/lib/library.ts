import { useEffect, useRef, useState } from "react";
import { create } from "zustand";
import { open as pickFiles } from "@tauri-apps/plugin-dialog";
import { api, errorText, plainError } from "./api";
import { allClips, blobUrl, stopAiRun, switchProject, useEditor } from "./store";
import type { Collection, Library, LibraryProject, Project, Said, Snapshot } from "./types";
import { MEDIA_EXTENSIONS, importFailures } from "../components/panel/assets";

type Sort = "opened" | "name";
const SORT_KEY = "nuzky.homeSort";

/** What opening another project asks while an agent is editing this one. */
export interface PendingSwitch {
  /** "open “Croatia day 3”" or "start a new project". */
  what: string;
  verb: "open" | "start";
  run: () => Promise<Snapshot>;
  /** Where to put the playhead once it is open. */
  atUs?: number;
}

interface LibraryState {
  /** null until the first list arrived. */
  projects: LibraryProject[] | null;
  collections: Collection[];
  /** "all" or a collection id. */
  filter: string;
  sort: Sort;
  /** Paths of the selected projects on the home screen. */
  selection: string[];
  /** The poster of each project, by path, with the file version it shows. */
  posters: Record<string, { version: number; url: string | null }>;
  pendingSwitch: PendingSwitch | null;
  /** A card to focus once it is listed, e.g. a duplicate. */
  focusPath: string | null;
  /** A project being renamed on its card. */
  renaming: string | null;
  /** Why the projects could not be listed, until a list arrives. */
  loadError: string | null;
  /** A search the launcher hands to the home screen with Show all results. */
  handoffQuery: string | null;
}

export const useLibrary = create<LibraryState>(() => ({
  projects: null,
  collections: [],
  filter: "all",
  sort: localStorage.getItem(SORT_KEY) === "name" ? "name" : "opened",
  selection: [],
  posters: {},
  pendingSwitch: null,
  focusPath: null,
  renaming: null,
  loadError: null,
  handoffQuery: null,
}));

export function setSort(sort: Sort) {
  localStorage.setItem(SORT_KEY, sort);
  useLibrary.setState({ sort });
}

const toast = (...args: Parameters<ReturnType<typeof useEditor.getState>["toast"]>) => useEditor.getState().toast(...args);
const failed = (e: unknown) => toast({ kind: "error", text: plainError(errorText(e)) });

let noticeShown = false;
let listing: Promise<void> | null = null;
let listAgain = false;
let listedAt = 0;

/** The list, listed once more while a newer one was asked for on its way: that one may predate a change. */
async function newestList(): Promise<Library> {
  for (;;) {
    listAgain = false;
    const library = await api.library().catch((e) => {
      if (!listAgain) throw e;
      return null;
    });
    if (library && !listAgain) return library;
  }
}

/**
 * Lists the projects again. A call made while a list is on its way lists once more after it, and
 * only that newer list shows, so a list started before a move to the Trash does not bring the
 * project back. `ifOlderThan` skips a list that fresh and shares one on its way.
 */
export function refreshLibrary(ifOlderThan = 0): Promise<void> {
  if (ifOlderThan > 0 && Date.now() - listedAt < ifOlderThan) return Promise.resolve();
  if (listing) {
    if (ifOlderThan === 0) listAgain = true;
    return listing;
  }
  listing = newestList()
    .then(
      (library) => {
        listedAt = Date.now();
        const paths = new Set(library.projects.map((p) => p.path));
        useLibrary.setState((s) => ({
          projects: library.projects,
          collections: library.collections,
          loadError: null,
          selection: s.selection.filter((p) => paths.has(p)),
          filter: s.filter === "all" || library.collections.some((c) => c.id === s.filter) ? s.filter : "all",
        }));
        if (library.notice && !noticeShown) {
          noticeShown = true;
          toast({ kind: "warning", text: library.notice });
        }
      },
      (e) => useLibrary.setState({ loadError: plainError(errorText(e)) }),
    )
    .finally(() => (listing = null));
  return listing;
}

/**
 * Ctrl+Z on the home screen: the newest toast's Undo, while it still applies. Trash, Move to
 * collection and Delete collection put one there.
 */
export function undoLastToast(): boolean {
  const { toasts, dismissToast } = useEditor.getState();
  const last = [...toasts].reverse().find((t) => t.action?.label === "Undo" && (t.action.valid?.() ?? true));
  if (!last?.action) return false;
  last.action.run();
  dismissToast(last.id);
  return true;
}

// --- Posters ------------------------------------------------------------------------------------

const posterQueue: LibraryProject[] = [];
const requested = new Map<string, number>();
let postersLoading = 0;

/** Asks for the project's poster once per file version; two render at a time. */
export function loadPoster(p: LibraryProject) {
  if (p.state === "broken" || p.state === "empty") return;
  if (useLibrary.getState().posters[p.path]?.version === p.modifiedMs || requested.get(p.path) === p.modifiedMs) return;
  requested.set(p.path, p.modifiedMs);
  posterQueue.push(p);
  pumpPosters();
}

function pumpPosters() {
  while (postersLoading < 2 && posterQueue.length > 0) {
    const p = posterQueue.shift()!;
    postersLoading++;
    api
      .projectPoster(p.path)
      .catch(() => null)
      .then((url) => {
        const old = useLibrary.getState().posters[p.path]?.url;
        if (old?.startsWith("blob:")) URL.revokeObjectURL(old);
        useLibrary.setState((s) => ({ posters: { ...s.posters, [p.path]: { version: p.modifiedMs, url: url ? blobUrl(url) : null } } }));
      })
      .finally(() => {
        postersLoading--;
        pumpPosters();
      });
  }
}

// --- Text ---------------------------------------------------------------------------------------

/** Lowercase without accents, so "zapad" finds "Západ". */
export const fold = (text: string) => text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

/** Every word of the query appears in the name or the collection's name. */
export function matches(p: LibraryProject, query: string, collections: Collection[]) {
  const words = fold(query).split(/\s+/).filter(Boolean);
  const name = fold(`${p.name} ${collections.find((c) => c.id === p.collection)?.name ?? ""}`);
  return words.every((w) => name.includes(w));
}

const DAY = 86_400_000;
const startOfDay = (ms: number) => new Date(new Date(ms).setHours(0, 0, 0, 0)).getTime();
const MONTH = new Intl.DateTimeFormat("en", { month: "long" });
const MONTH_YEAR = new Intl.DateTimeFormat("en", { month: "long", year: "numeric" });
const WEEKDAY = new Intl.DateTimeFormat("en", { weekday: "long" });
const SHORT = new Intl.DateTimeFormat("en", { month: "short", day: "numeric" });
const SHORT_YEAR = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", year: "numeric" });

/** The group a project falls in when sorted by when it was last opened or changed, as Finder groups files. */
export function periodOf(ms: number, now = Date.now()): string {
  const days = Math.round((startOfDay(now) - startOfDay(ms)) / DAY);
  if (days <= 0) return "Today";
  if (days < 7) return "Previous 7 days";
  if (days < 30) return "Previous 30 days";
  return new Date(ms).getFullYear() === new Date(now).getFullYear() ? MONTH.format(ms) : MONTH_YEAR.format(ms);
}

/** When a project was last opened or changed: "12 min ago", "Yesterday", "Monday", "Sep 28". */
export function whenLabel(ms: number, now = Date.now()): string {
  const days = Math.round((startOfDay(now) - startOfDay(ms)) / DAY);
  if (days <= 0) {
    const minutes = Math.floor((now - ms) / 60_000);
    if (minutes < 1) return "Just now";
    if (minutes < 60) return `${minutes} min ago`;
    return `${Math.floor(minutes / 60)} h ago`;
  }
  if (days === 1) return "Yesterday";
  if (days < 7) return WEEKDAY.format(ms);
  return new Date(ms).getFullYear() === new Date(now).getFullYear() ? SHORT.format(ms) : SHORT_YEAR.format(ms);
}

/** Why a project cannot be opened here, for its card and its tooltip. */
export function blocked(p: LibraryProject): string | null {
  if (p.state === "busy") return "This project is open in another Nuzky window or an AI agent is editing it. Close it there first.";
  if (p.state === "broken") return `This file can't be opened. It may be damaged, or saved by a newer Nuzky. ${p.error ?? ""}`.trim();
  return null;
}

/** Nothing in it yet: what New project and the first start create. */
function pristine(project: Project) {
  return project.name === "Untitled project" && project.assets.length === 0 && allClips(project).length === 0;
}

/** No projects but the empty one Nuzky made: the home screen offers formats to start with. */
export function onlyPristine(projects: LibraryProject[] | null, snap: Snapshot | null) {
  return projects !== null && projects.every((p) => !!snap && p.path === snap.path && pristine(snap.project));
}

// --- Opening and creating -----------------------------------------------------------------------

/** Back to the open project, at `atUs` when given; words found there show in the Transcript tab. */
export function showEditor(atUs?: number) {
  useEditor.setState({ view: "editor", launcherOpen: false });
  if (atUs !== undefined) {
    useEditor.getState().seek(atUs);
    useEditor.setState({ panelTab: "transcript" });
  }
}

export function showHome() {
  if (useEditor.getState().playing) void api.pause();
  useEditor.setState({ view: "home", launcherOpen: false });
}

/** While an agent edits, asks what happens to its changes first (`resolveSwitch`). */
function switchTo(pending: PendingSwitch) {
  if (useEditor.getState().aiRun) useLibrary.setState({ pendingSwitch: pending });
  else void finishSwitch(pending);
}

let switching: Promise<void> = Promise.resolve();

/** One switch at a time, in the order asked, so the project chosen last is the one left open. */
function finishSwitch(pending: PendingSwitch): Promise<void> {
  switching = switching.then(() => runSwitch(pending)).catch(failed);
  return switching;
}

async function runSwitch(pending: PendingSwitch) {
  const filter = useLibrary.getState().filter;
  try {
    await switchProject(pending.run);
  } catch (e) {
    failed(e);
    void refreshLibrary();
    return;
  }
  useEditor.setState({ timeUs: 0 });
  showEditor(pending.atUs);
  // A project started while a collection is shown belongs to it.
  const path = useEditor.getState().snap?.path;
  if (pending.verb === "start" && filter !== "all" && path) await api.setCollection([path], filter).catch(failed);
  void refreshLibrary();
}

/** The answer to `pendingSwitch`: keep or undo the agent's changes, then switch; or stay. */
export async function resolveSwitch(choice: "keep" | "undo" | "cancel") {
  const pending = useLibrary.getState().pendingSwitch;
  useLibrary.setState({ pendingSwitch: null });
  if (!pending || choice === "cancel") return;
  if (await stopAiRun(choice === "undo")) await finishSwitch(pending);
}

export function openProject(p: Pick<LibraryProject, "path" | "name">, atUs?: number) {
  if (useEditor.getState().snap?.path === p.path) return showEditor(atUs);
  switchTo({ what: `open “${p.name}”`, verb: "open", run: () => api.openProject(p.path), atUs });
}

/** An empty untitled project open now takes the format instead of leaving a second empty file. */
export function newProject(width: number, height: number) {
  const { snap, aiRun, edit } = useEditor.getState();
  if (snap && !aiRun && pristine(snap.project)) {
    const filter = useLibrary.getState().filter;
    void edit({ type: "setCanvas", width, height });
    if (filter !== "all") void api.setCollection([snap.path], filter).then(() => refreshLibrary(), failed);
    return showEditor();
  }
  switchTo({ what: "start a new project", verb: "start", run: () => api.newProject(width, height) });
}

/** A project of these videos, in this order, in the format of the first one. */
export function newProjectFromMedia(paths: string[]) {
  switchTo({
    what: "start a new project",
    verb: "start",
    run: async () => {
      const result = await api.newProjectFromMedia(paths);
      if (result.failed.length > 0) toast({ kind: "error", text: importFailures(result.failed) });
      return result.snapshot;
    },
  });
}

export async function pickVideosForNewProject() {
  const picked = await pickFiles({ multiple: true, filters: [{ name: "Video, audio and images", extensions: MEDIA_EXTENSIONS }] });
  if (picked && picked.length > 0) newProjectFromMedia(Array.isArray(picked) ? picked : [picked]);
}

export async function openProjectFile() {
  const picked = await pickFiles({ multiple: false, filters: [{ name: "Nuzky project", extensions: ["nuzky"] }] });
  if (typeof picked === "string") openProject({ path: picked, name: picked.split(/[\\/]/).pop()?.replace(/\.nuzky$/, "") ?? "project" });
}

// --- Managing -----------------------------------------------------------------------------------

const projectName = (path: string) => useLibrary.getState().projects?.find((p) => p.path === path)?.name ?? "project";
const collectionName = (id: string | null) => useLibrary.getState().collections.find((c) => c.id === id)?.name;

export async function renameProject(p: LibraryProject, name: string) {
  name = name.trim();
  if (!name || name === p.name) return;
  // The open project is renamed by an edit, so Undo in the editor takes it back.
  if (p.path === useEditor.getState().snap?.path) await useEditor.getState().edit({ type: "renameProject", name });
  else await api.renameProject(p.path, name).catch(failed);
  await refreshLibrary();
}

export async function duplicateProject(p: LibraryProject) {
  try {
    const path = await api.duplicateProject(p.path);
    useLibrary.setState({ focusPath: path, selection: [] });
  } catch (e) {
    failed(e);
  }
  await refreshLibrary();
}

/** Hides them at once; Undo in the toast brings them back until they really go to the Trash. */
export async function trashProjects(paths: string[]) {
  if (paths.length === 0) return;
  const text =
    paths.length === 1 ? `Moved “${projectName(paths[0])}” to the Trash. Your videos stay where they are.` : `Moved ${paths.length} projects to the Trash. Your videos stay where they are.`;
  try {
    await api.trashProjects(paths);
  } catch (e) {
    return failed(e);
  }
  useLibrary.setState((s) => ({ projects: s.projects?.filter((p) => !paths.includes(p.path)) ?? null, selection: s.selection.filter((p) => !paths.includes(p)) }));
  void refreshLibrary();
  toast({
    kind: "info",
    text,
    action: {
      label: "Undo",
      run: () => void api.restoreProjects(paths).then(() => refreshLibrary(), failed),
    },
  });
}

export async function moveToCollection(paths: string[], collection: string | null) {
  const before = new Map((useLibrary.getState().projects ?? []).filter((p) => paths.includes(p.path)).map((p) => [p.path, p.collection]));
  try {
    await api.setCollection(paths, collection);
  } catch (e) {
    return failed(e);
  }
  useLibrary.setState((s) => ({ projects: s.projects?.map((p) => (paths.includes(p.path) ? { ...p, collection } : p)) ?? null }));
  void refreshLibrary();
  const what = paths.length === 1 ? `“${projectName(paths[0])}”` : `${paths.length} projects`;
  const from = collectionName([...before.values()][0] ?? null);
  toast({
    kind: "success",
    text: collection ? `Moved ${what} to ${collectionName(collection)}.` : `Took ${what} out of ${from ?? "its collection"}.`,
    action: {
      label: "Undo",
      run: async () => {
        // Back to where each was, one call per collection.
        const groups = new Map<string | null, string[]>();
        for (const [path, was] of before) groups.set(was, [...(groups.get(was) ?? []), path]);
        for (const [was, group] of groups) await api.setCollection(group, was).catch(failed);
        await refreshLibrary();
      },
    },
  });
}

export async function createCollection(name: string, paths: string[] = []): Promise<Collection | null> {
  try {
    const collection = await api.createCollection(name);
    if (paths.length > 0) await api.setCollection(paths, collection.id);
    await refreshLibrary();
    return collection;
  } catch (e) {
    failed(e);
    return null;
  }
}

export async function renameCollection(id: string, name: string) {
  if (!name.trim() || name.trim() === collectionName(id)) return;
  await api.renameCollection(id, name).catch(failed);
  await refreshLibrary();
}

export async function deleteCollection(id: string) {
  try {
    const deleted = await api.deleteCollection(id);
    await refreshLibrary();
    toast({
      kind: "info",
      text: `Deleted the collection “${deleted.collection.name}”. Its projects are still in All projects.`,
      action: { label: "Undo", run: () => void api.restoreCollection(deleted).then(() => refreshLibrary(), failed) },
    });
  } catch (e) {
    failed(e);
  }
}

// --- Search through what was said ----------------------------------------------------------------

/**
 * Where the words of `query` were said, in the projects of `collection` (all with null), searched
 * shortly after typing stops. `result` belongs to `query` only once it arrived; until then
 * `searching` is true and the last result stays.
 */
export function useSaidSearch(query: string, collection: string | null = null) {
  const [state, setState] = useState<{ query: string; result: Said | null; searching: boolean }>({ query: "", result: null, searching: false });
  const seq = useRef(0);
  useEffect(() => {
    const id = ++seq.current;
    if (fold(query).replace(/\s+/g, "").length < 2) {
      setState({ query, result: null, searching: false });
      return;
    }
    setState((s) => ({ ...s, searching: true }));
    const timer = window.setTimeout(() => {
      api.searchSaid(query, collection).then(
        (result) => id === seq.current && setState({ query, result, searching: false }),
        (e) => {
          if (id !== seq.current) return;
          setState({ query, result: null, searching: false });
          failed(e);
        },
      );
    }, 160);
    return () => window.clearTimeout(timer);
  }, [query, collection]);
  return state;
}
