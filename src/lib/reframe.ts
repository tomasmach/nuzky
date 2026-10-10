import { create } from "zustand";
import { api, errorText, plainError } from "./api";
import { formatLabel } from "./presets";
import { aiLocked, currentEpoch, useEditor, whenIdle } from "./store";
import type { JobEvent, Project, Snapshot } from "./types";

interface ReframeState {
  /** The running reframe. */
  job: string | null;
  /** What the last reframe did, offered for Undo while it is the newest step. */
  done: { snap: Snapshot; text: string } | null;
}

export const useReframe = create<ReframeState>(() => ({ job: null, done: null }));

/** The video and image clips among `ids`. */
export function picturedClips(project: Project, ids: string[]): string[] {
  return project.tracks
    .filter((t) => t.kind === "video")
    .flatMap((t) => t.clips)
    .filter((c) => c.content.type === "media" && ids.includes(c.id))
    .map((c) => c.id);
}

/**
 * Changes the canvas to `width`×`height` and places the selected video and image clips, or every one when none is
 * selected, to follow the face. The job applies it as one undo step; the preview then offers Undo.
 */
export async function reframe(width: number, height: number) {
  if (aiLocked() || useReframe.getState().job) return;
  // Edits made just before are part of what is reframed, not a change that would make it fail.
  await whenIdle();
  const { snap, selection, toast } = useEditor.getState();
  if (!snap) return;
  const ids = picturedClips(snap.project, selection);
  useReframe.setState({ done: null });
  try {
    useReframe.setState({ job: await api.startReframe(width, height, ids.length > 0 ? ids : null, currentEpoch()) });
  } catch (e) {
    toast({ kind: "error", text: plainError(errorText(e)) });
  }
}

export function onReframeJob(job: JobEvent) {
  if (job.id !== useReframe.getState().job || job.status === "running") return;
  useReframe.setState({ job: null });
  const { snap, toast } = useEditor.getState();
  if (job.status === "done" && snap && job.output) {
    const out = JSON.parse(job.output) as { width: number; height: number; followed: number; centred: number };
    const clips = out.followed + out.centred;
    const text =
      `Reframed ${clips} clip${clips === 1 ? "" : "s"} to ${formatLabel(out.width, out.height)}` +
      (out.centred > 0 ? `, ${out.centred} without a face centred` : "");
    // The edit arrived just before its job ended, so this is its snapshot.
    useReframe.setState({ done: { snap, text } });
  }
  if (job.status === "failed") {
    const message = job.message ?? "";
    toast({
      kind: "error",
      text: message.includes("VISION_UNAVAILABLE") ? "Faces can't be found on this computer, so the clips were not reframed." : `Reframe failed: ${plainError(message)}`,
    });
  }
}
