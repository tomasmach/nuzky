import { useEffect, useRef } from "react";
import { useEditor } from "../../lib/store";
import { US } from "../../lib/time";

const PEAKS_PER_SECOND = 50;

/** Peak bars for the source range a clip plays; `speed` stretches it like the renderer. */
export function Waveform({
  assetId,
  sourceInUs,
  durationUs,
  speed,
  width,
  color,
  className = "absolute inset-x-0 bottom-0 h-[60%]",
}: {
  assetId: string;
  sourceInUs: number;
  durationUs: number;
  speed: number;
  width: number;
  color: string;
  className?: string;
}) {
  const peaks = useEditor((s) => s.waveforms[assetId]);
  const ref = useRef<HTMLCanvasElement>(null);
  const epoch = useEditor((s) => s.snap?.sessionEpoch);
  useEffect(() => useEditor.getState().loadWaveform(assetId), [assetId, epoch]);
  useEffect(() => {
    const el = ref.current;
    if (!el || !peaks) return;
    const w = Math.max(1, Math.min(4096, Math.round(width)));
    const h = el.clientHeight || 24;
    el.width = w;
    el.height = h;
    const ctx = el.getContext("2d")!;
    ctx.clearRect(0, 0, w, h);
    ctx.fillStyle = color;
    const first = (sourceInUs / US) * PEAKS_PER_SECOND;
    const span = ((durationUs * speed) / US) * PEAKS_PER_SECOND;
    for (let x = 0; x < w; x++) {
      const a = Math.floor(first + (x / w) * span);
      const b = Math.max(a + 1, Math.floor(first + ((x + 1) / w) * span));
      let m = 0;
      for (let i = a; i < b && i < peaks.length; i++) m = Math.max(m, peaks[i]);
      // Square root keeps quiet audio visible, roughly like a dB scale.
      const bar = Math.max(1, Math.sqrt(m / 255) * h);
      ctx.fillRect(x, h - bar, 1, bar);
    }
  }, [peaks, sourceInUs, durationUs, speed, width, color]);
  if (!peaks) return null;
  return <canvas ref={ref} className={`pointer-events-none w-full opacity-80 ${className}`} />;
}
