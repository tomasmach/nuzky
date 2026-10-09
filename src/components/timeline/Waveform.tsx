import { useEffect, useRef } from "react";
import { PEAKS_PER_BLOCK, PEAKS_PER_SECOND, useEditor, type WaveBlock } from "../../lib/store";
import { US } from "../../lib/time";

/** The widest canvas drawn; the timeline draws only its window of a clip. */
const MAX_WIDTH = 8192;

/** A peak, or undefined while it is not loaded. Past the end of a finished block the sound has ended. */
function peakAt(blocks: Record<number, WaveBlock> | undefined, i: number): number | undefined {
  const block = blocks?.[Math.floor(i / PEAKS_PER_BLOCK)];
  if (!block) return undefined;
  const j = i % PEAKS_PER_BLOCK;
  return j < block.peaks.length ? block.peaks[j] : block.complete ? 0 : undefined;
}

/**
 * Peak bars, centred on the middle line, for the source range a clip plays; `speed` stretches it like the renderer.
 * Only `visible` (px of the clip) is drawn, a bar per pixel, from the blocks of peaks under it and one block around;
 * pixels whose peaks are not loaded yet stay empty.
 */
export function Waveform({
  assetId,
  sourceInUs,
  durationUs,
  speed,
  width,
  visible,
  color,
  className = "absolute top-px h-[calc(100%-2px)]",
}: {
  assetId: string;
  sourceInUs: number;
  durationUs: number;
  speed: number;
  width: number;
  visible?: [number, number];
  color: string;
  className?: string;
}) {
  const blocks = useEditor((s) => s.waveforms[assetId]);
  const ref = useRef<HTMLCanvasElement>(null);
  const epoch = useEditor((s) => s.snap?.sessionEpoch);
  const first = (sourceInUs / US) * PEAKS_PER_SECOND;
  const span = ((durationUs * speed) / US) * PEAKS_PER_SECOND;
  const from = Math.max(0, Math.floor(visible?.[0] ?? 0));
  const to = Math.min(Math.ceil(width), Math.ceil(visible?.[1] ?? width), from + MAX_WIDTH);
  const shown = to > from && width > 0;
  const firstBlock = Math.max(0, Math.floor((first + (from / width) * span) / PEAKS_PER_BLOCK) - 1);
  const lastBlock = Math.floor((first + (to / width) * span) / PEAKS_PER_BLOCK) + 1;
  useEffect(() => (shown ? useEditor.getState().showWaveform(assetId, firstBlock, lastBlock) : undefined), [assetId, firstBlock, lastBlock, shown, epoch]);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const w = to - from;
    const h = el.clientHeight || 24;
    el.width = w;
    el.height = h;
    const ctx = el.getContext("2d")!;
    ctx.fillStyle = color;
    // Bars never overlap, so alpha here looks the same as CSS opacity and spares a transparency layer per canvas.
    ctx.globalAlpha = 0.8;
    for (let x = 0; x < w; x++) {
      const a = Math.floor(first + ((from + x) / width) * span);
      const b = Math.max(a + 1, Math.floor(first + ((from + x + 1) / width) * span));
      let m = 0;
      for (let i = a; i < b && m >= 0; i++) {
        const peak = peakAt(blocks, i);
        m = peak === undefined ? -1 : Math.max(m, peak);
      }
      if (m < 0) continue;
      // Square root keeps quiet audio visible, roughly like a dB scale.
      const bar = Math.max(1, Math.round(Math.sqrt(m / 255) * h));
      ctx.fillRect(x, (h - bar) >> 1, 1, bar);
    }
  }, [blocks, from, to, first, span, width, color]);
  if (!shown) return null;
  return <canvas ref={ref} className={`pointer-events-none ${className}`} style={{ left: from, width: to - from }} />;
}
