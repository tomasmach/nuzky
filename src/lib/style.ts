import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { api, errorText, plainError } from "./api";
import { showHome, useLibrary } from "./library";
import { useEditor } from "./store";
import type { JobEvent, StyleAction, StylePair, StylePairResult, StyleTimelineResult, StyleView, TimelinePlan } from "./types";

/** A job learning from videos: its pairs and how each ended, in order. */
export type Learning = { jobId: string; pairs: StylePair[]; results: (StylePairResult | null)[] };
/** A job learning from projects cut in another editor: what each was read as and how it ended. */
export type ProjectsLearning = { jobId: string; plans: TimelinePlan[]; results: (StyleTimelineResult | null)[] };
export type LearnTab = "videos" | "projects";

type StyleState = {
  /** null until read. */
  view: StyleView | null;
  /** Why the style could not be read. */
  error: string | null;
  learning: Learning | null;
  projectsLearning: ProjectsLearning | null;
  learnOpen: boolean;
  learnTab: LearnTab;
  /** The pairs chosen in the Learn dialog, kept while it is closed. */
  draft: StylePair[];
  /** The projects chosen in the Learn dialog, as read, kept while it is closed. */
  projects: TimelinePlan[];
  /** The AI panel beside the page, to talk the style through. */
  chatOpen: boolean;
};

export const NO_PAIR: StylePair = { recording: "", cut: "" };

export const useStyle = create<StyleState>(() => ({
  view: null,
  error: null,
  learning: null,
  projectsLearning: null,
  learnOpen: false,
  learnTab: "videos",
  draft: [NO_PAIR],
  projects: [],
  chatOpen: false,
}));

export const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

export async function loadStyle() {
  try {
    const before = useStyle.getState().view;
    const view = await api.styleView();
    useStyle.setState({ view, error: null });
    // A project Nuzky learned from by itself is news when it brought new suggestions.
    const learned = view.sources[0];
    if (before && learned?.kind === "project" && learned.atMs !== before.sources[0]?.atMs && view.suggestions.length > before.suggestions.length) {
      const more = view.suggestions.length - before.suggestions.length;
      useEditor.getState().toast({ kind: "info", text: `Learned from ${learned.title}: ${plural(more, "new suggestion")}`, action: { label: "Review", run: openStyle } });
    }
    // A change the AI made, in the panel or anywhere else, is news with Undo; the creator's own are not.
    const newest = view.versions[0];
    if (before && newest && newest.index > before.version && newest.label.startsWith("AI: ")) {
      useEditor.getState().toast({
        kind: "success",
        text: `The AI changed your style: ${newest.label.slice(4)}`,
        action: { label: "Undo", run: () => void changeStyle({ type: "restore", index: before.version }) },
      });
    }
  } catch (e) {
    useStyle.setState({ error: plainError(errorText(e)) });
  }
}

/** Changes the style. A failure is a toast and leaves the page as it was; true when it happened. */
export async function changeStyle(action: StyleAction, done?: string): Promise<boolean> {
  try {
    useStyle.setState({ view: await api.styleAct(action), error: null });
    if (done) useEditor.getState().toast({ kind: "success", text: done });
    return true;
  } catch (e) {
    useEditor.getState().toast({ kind: "error", text: plainError(errorText(e)) });
    return false;
  }
}

export async function startLearning(pairs: StylePair[]) {
  try {
    const jobId = await api.startStyleLearning(pairs);
    useStyle.setState({ learning: { jobId, pairs, results: pairs.map(() => null) } });
  } catch (e) {
    useEditor.getState().toast({ kind: "error", text: plainError(errorText(e)) });
  }
}

/** Reads timeline files into the Learn dialog's projects; a file chosen again is read again. */
export async function addProjects(paths: string[]) {
  try {
    const plans = await api.styleReadTimelines(paths);
    useStyle.setState((s) => ({ projects: [...s.projects.filter((p) => !paths.includes(p.path)), ...plans] }));
  } catch (e) {
    useEditor.getState().toast({ kind: "error", text: plainError(errorText(e)) });
  }
}

export async function startProjectsLearning(plans: TimelinePlan[]) {
  try {
    const jobId = await api.startTimelineLearning(plans.map((p) => p.path));
    useStyle.setState({ projectsLearning: { jobId, plans, results: plans.map(() => null) } });
  } catch (e) {
    useEditor.getState().toast({ kind: "error", text: plainError(errorText(e)) });
  }
}

/**
 * Makes what the projects taught the style, over the version `seen` the creator saw; true when it did.
 * When the style changed meanwhile, the page shows it as it is now and nothing is written.
 */
export async function saveLearned(jobId: string, seen: number): Promise<boolean> {
  const toast = useEditor.getState().toast;
  try {
    useStyle.setState({ view: await api.styleUseLearned(jobId, seen), error: null, projectsLearning: null, projects: [] });
    toast({ kind: "success", text: "Saved as your style", action: { label: "Undo", run: () => void changeStyle({ type: "restore", index: seen }) } });
    return true;
  } catch (e) {
    const text = errorText(e);
    if (text.startsWith("STYLE_CHANGED")) {
      await loadStyle();
      toast({ kind: "info", text: "Your style changed meanwhile. Look at it again before you replace it." });
    } else toast({ kind: "error", text: plainError(text) });
    return false;
  }
}

export function openStyle() {
  useLibrary.setState({ page: "style", selection: [] });
  showHome();
}


/** Keeps the style current as learning finds things, and reports a learning job that ends with its dialog closed. */
export function listenStyle() {
  const offs = [
    listen("style-changed", () => void loadStyle()),
    listen<StylePairResult>("style-pair", (e) => {
      const learning = useStyle.getState().learning;
      if (learning?.jobId !== e.payload.jobId) return;
      const results = learning.results.map((r, i) => (i === e.payload.index ? e.payload : r));
      useStyle.setState({ learning: { ...learning, results } });
    }),
    listen<StyleTimelineResult>("style-timeline", (e) => {
      const learning = useStyle.getState().projectsLearning;
      if (learning?.jobId !== e.payload.jobId) return;
      const results = learning.results.map((r, i) => (i === e.payload.index ? e.payload : r));
      useStyle.setState({ projectsLearning: { ...learning, results } });
    }),
    listen<JobEvent>("job", async (e) => {
      const job = e.payload;
      if (job.kind !== "style" || job.status === "running") return;
      await loadStyle();
      const { learning, projectsLearning, learnOpen, view } = useStyle.getState();
      const projects = projectsLearning?.jobId === job.id;
      if (learnOpen || (learning?.jobId !== job.id && !projects)) return;
      const toast = useEditor.getState().toast;
      const details = { label: "Details", run: () => useStyle.setState({ learnOpen: true, learnTab: projects ? "projects" : "videos" }) };
      if (job.status === "done" && projects) {
        const learned = projectsLearning.results.filter((r) => r?.learned.length).length;
        toast({ kind: "success", text: `Learned from ${plural(learned, "project")}`, action: { ...details, label: "Review" } });
      } else if (job.status === "done" && learning) {
        const learned = learning.results.filter((r) => r && !r.error).length;
        const found = view?.suggestions.length ?? 0;
        toast({ kind: "success", text: `Learned from ${plural(learned, "video")}: ${plural(found, "suggestion")}`, action: { label: "Review", run: openStyle } });
      }
      if (job.status === "failed") toast({ kind: "error", text: plainError(job.message ?? "Learning failed"), action: details });
    }),
  ];
  return () => offs.forEach((off) => void off.then((f) => f()));
}
