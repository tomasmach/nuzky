import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { api, errorText, plainError } from "./api";
import { showHome, useLibrary } from "./library";
import { useEditor } from "./store";
import type { JobEvent, StyleAction, StylePair, StylePairResult, StyleView } from "./types";

/** A "Learn from videos" job: its pairs and how each ended, in order. */
export type Learning = { jobId: string; pairs: StylePair[]; results: (StylePairResult | null)[] };

type StyleState = {
  /** null until read. */
  view: StyleView | null;
  /** Why the style could not be read. */
  error: string | null;
  learning: Learning | null;
  learnOpen: boolean;
  /** The pairs chosen in the Learn dialog, kept while it is closed. */
  draft: StylePair[];
  /** The AI panel beside the page, to talk the style through. */
  chatOpen: boolean;
};

export const NO_PAIR: StylePair = { recording: "", cut: "" };

export const useStyle = create<StyleState>(() => ({ view: null, error: null, learning: null, learnOpen: false, draft: [NO_PAIR], chatOpen: false }));

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
    listen<JobEvent>("job", async (e) => {
      const job = e.payload;
      if (job.kind !== "style" || job.status === "running") return;
      await loadStyle();
      const { learning, learnOpen, view } = useStyle.getState();
      if (learnOpen || learning?.jobId !== job.id) return;
      const toast = useEditor.getState().toast;
      if (job.status === "done") {
        const learned = learning.results.filter((r) => r && !r.error).length;
        const found = view?.suggestions.length ?? 0;
        toast({ kind: "success", text: `Learned from ${plural(learned, "video")}: ${plural(found, "suggestion")}`, action: { label: "Review", run: openStyle } });
      }
      if (job.status === "failed") toast({ kind: "error", text: plainError(job.message ?? "Learning failed"), action: { label: "Details", run: () => useStyle.setState({ learnOpen: true }) } });
    }),
  ];
  return () => offs.forEach((off) => void off.then((f) => f()));
}
