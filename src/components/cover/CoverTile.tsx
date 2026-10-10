import { useEffect, useRef, useState } from "react";
import { ImagePlus } from "lucide-react";
import { api } from "../../lib/api";
import { closeCover, openCover, thumbnailOf, useCover } from "../../lib/cover";
import { projectDuration, useEditor } from "../../lib/store";
import type { ThumbnailFormat } from "../../lib/types";

const W = 44;
const H = 56;

/**
 * The Cover tile at the start of the main track, as in CapCut: the cover as it exports, or an empty shape
 * until there is one. It opens and closes the cover editor.
 */
export function CoverTile() {
  const open = useCover((s) => s.open);
  const empty = useEditor((s) => !s.snap || projectDuration(s.snap.project) <= 0);
  const format = useEditor((s): ThumbnailFormat | null =>
    thumbnailOf(s.snap?.project, "cover_9x16") ? "cover_9x16" : thumbnailOf(s.snap?.project, "youtube_16x9") ? "youtube_16x9" : null,
  );
  const revision = useEditor((s) => s.snap?.revision ?? 0);
  // Every project starts at revision 0, so the session tells two projects apart.
  const epoch = useEditor((s) => s.snap?.sessionEpoch);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [drawn, setDrawn] = useState(false);

  // Another project's cover goes at once, before this project's arrives.
  useEffect(() => {
    const el = canvas.current;
    el?.getContext("2d")?.clearRect(0, 0, el.width, el.height);
    setDrawn(false);
  }, [epoch]);

  // Drawn by the engine like the export, a moment after the project settles. While the cover editor shows the
  // other format it waits, so the two do not take turns decoding their frames.
  useEffect(() => {
    if (!format || (open && open !== format)) return;
    let alive = true;
    const timer = window.setTimeout(() => {
      api
        .coverView(format, (format === "cover_9x16" ? W : (H * 16) / 9) * 2, false, null)
        .then(({ view, rgba }) => {
          const el = canvas.current;
          if (!alive || !el) return;
          el.width = view.width;
          el.height = view.height;
          el.getContext("2d")?.putImageData(new ImageData(rgba, view.width, view.height), 0, 0);
          setDrawn(true);
        })
        .catch(() => alive && setDrawn(false));
    }, 600);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [format, revision, open, epoch]);

  const label = empty ? "Cover: add a clip to the timeline first" : open ? "Close the cover editor" : format ? "Edit the cover" : "Make a cover";
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={!!open}
      aria-disabled={empty || undefined}
      onClick={empty ? undefined : () => (open ? closeCover() : openCover())}
      data-testid="cover-tile"
      className={`relative shrink-0 overflow-hidden rounded-[7px] aria-disabled:cursor-not-allowed aria-disabled:opacity-40 ${
        open ? "shadow-[0_0_0_2px_var(--color-accent)]" : format ? "shadow-[inset_0_0_0_1px_rgb(255_255_255/.1)] hover:shadow-[inset_0_0_0_1px_rgb(255_255_255/.3)]" : "border border-dashed border-white/15 hover:border-white/30"
      }`}
      style={{ width: W, height: H }}
    >
      {format && <canvas ref={canvas} className={`absolute inset-0 h-full w-full object-cover ${drawn ? "" : "bg-white/[.04]"}`} />}
      {!format && <ImagePlus size={15} className="absolute left-1/2 top-[14px] -translate-x-1/2 text-muted" />}
      <span className="absolute inset-x-0.5 bottom-0.5 rounded-[4px] bg-black/60 text-center text-[11px] leading-[14px] text-fg">Cover</span>
    </button>
  );
}
