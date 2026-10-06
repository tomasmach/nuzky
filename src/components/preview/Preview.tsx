import { useEffect, useRef, useState } from "react";
import { Pause, Play, SkipBack, SkipForward, Smartphone } from "lucide-react";
import { api } from "../../lib/api";
import { projectDuration, useEditor } from "../../lib/store";
import { safeArea } from "../../lib/presets";
import { formatTime } from "../../lib/time";
import { IconButton } from "../ui";
import { LayerOverlay } from "./LayerOverlay";
import { RatioMenu } from "./RatioMenu";

const HEADER = 24;
const SAFE_ZONE_KEY = "capopen.safeZone";

/** The parts of the frame Reels and TikTok cover with their interface, dimmed, around the free area. */
function SafeZone({ area, width, height }: { area: NonNullable<ReturnType<typeof safeArea>>; width: number; height: number }) {
  const pct = (v: number, of: number) => `${(v / of) * 100}%`;
  const band = "pointer-events-none absolute bg-black/45";
  return (
    <>
      <div className={`${band} inset-x-0 top-0`} style={{ height: pct(area.top, height) }} />
      <div className={`${band} inset-x-0 bottom-0`} style={{ height: pct(height - area.bottom, height) }} />
      <div className={`${band} left-0`} style={{ top: pct(area.top, height), bottom: pct(height - area.bottom, height), width: pct(area.left, width) }} />
      <div className={`${band} right-0`} style={{ top: pct(area.top, height), bottom: pct(height - area.bottom, height), width: pct(width - area.right, width) }} />
      <div
        className="pointer-events-none absolute border border-dashed border-fg/50"
        style={{ left: pct(area.left, width), top: pct(area.top, height), width: pct(area.right - area.left, width), height: pct(area.bottom - area.top, height) }}
      />
    </>
  );
}

/** Receives rendered frames from the engine and paints the newest one per display frame. */
function useFrameStream(url: string, canvas: React.RefObject<HTMLCanvasElement | null>) {
  const [connected, setConnected] = useState(false);
  useEffect(() => {
    if (!url) return;
    let ws: WebSocket | null = null;
    let closed = false;
    let latest: ArrayBuffer | null = null;
    let raf = 0;
    let retry = 0;

    const draw = () => {
      raf = 0;
      const buf = latest;
      latest = null;
      const el = canvas.current;
      if (!buf || !el) return;
      const view = new DataView(buf);
      if (view.getUint32(0, true) !== 0x31465043) return; // "CPF1"
      const w = view.getUint32(4, true);
      const h = view.getUint32(8, true);
      const playing = (view.getUint32(12, true) & 1) === 1;
      const t = Number(view.getBigInt64(16, true));
      if (el.width !== w || el.height !== h) {
        el.width = w;
        el.height = h;
      }
      el.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(buf, HEADER, w * h * 4), w, h), 0, 0);
      if (playing) useEditor.setState({ timeUs: t });
    };

    const connect = () => {
      ws = new WebSocket(url);
      ws.binaryType = "arraybuffer";
      ws.onopen = () => {
        retry = 0;
        setConnected(true);
      };
      ws.onmessage = (e) => {
        latest = e.data as ArrayBuffer;
        if (!raf) raf = requestAnimationFrame(draw);
      };
      ws.onclose = () => {
        setConnected(false);
        if (!closed) window.setTimeout(connect, Math.min(2000, 200 * ++retry));
      };
    };
    connect();
    return () => {
      closed = true;
      ws?.close();
      cancelAnimationFrame(raf);
    };
  }, [url, canvas]);
  return connected;
}

export function Preview() {
  const url = useEditor((s) => s.previewUrl);
  const canvas = useEditor((s) => s.snap?.project.canvas);
  const duration = useEditor((s) => (s.snap ? projectDuration(s.snap.project) : 0));
  const playing = useEditor((s) => s.playing);
  const timeUs = useEditor((s) => s.timeUs);
  const engineError = useEditor((s) => s.engineError);
  const { togglePlay, seek } = useEditor.getState();
  const boxRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [box, setBox] = useState({ w: 0, h: 0 });
  const connected = useFrameStream(url, canvasRef);

  useEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    let timer = 0;
    const ro = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      setBox({ w: width, h: height });
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        const dpr = window.devicePixelRatio || 1;
        api.setPreviewBox(Math.round(width * dpr), Math.round(height * dpr));
      }, 80);
    });
    ro.observe(el);
    return () => {
      ro.disconnect();
      window.clearTimeout(timer);
    };
  }, []);

  const aspect = canvas ? canvas.width / canvas.height : 9 / 16;
  const area = canvas ? safeArea(canvas.width, canvas.height) : null;
  const [showSafe, setShowSafe] = useState(() => localStorage.getItem(SAFE_ZONE_KEY) === "1");
  const toggleSafe = () => {
    localStorage.setItem(SAFE_ZONE_KEY, showSafe ? "0" : "1");
    setShowSafe(!showSafe);
  };
  const fit = box.w / box.h > aspect ? { h: box.h, w: box.h * aspect } : { w: box.w, h: box.w / aspect };
  const empty = duration === 0;

  return (
    <section className="flex min-w-0 flex-1 flex-col bg-bg" aria-label="Preview">
      {/* Clips the selection box of a layer scaled or rotated past the frame to the preview area. */}
      <div ref={boxRef} className="relative m-3 mb-0 min-h-0 flex-1 overflow-hidden">
        <div className="absolute inset-0 flex items-center justify-center">
          <div className="relative" style={{ width: fit.w, height: fit.h }}>
            <div className="absolute inset-0 overflow-hidden rounded-sm bg-black shadow-[0_0_0_1px_var(--color-line)]">
              <canvas ref={canvasRef} className="h-full w-full" style={{ imageRendering: "auto" }} />
              {showSafe && area && canvas && <SafeZone area={area} width={canvas.width} height={canvas.height} />}
              {empty && (
                <div className="absolute inset-0 flex flex-col items-center justify-center gap-1 p-6 text-center">
                  <p className="text-[13px] text-fg">Your video appears here</p>
                  <p className="text-[12px] text-muted">Add media to the timeline to start editing.</p>
                </div>
              )}
              {!connected && !engineError && <div className="skeleton absolute inset-0 opacity-60" aria-label="Connecting to preview" />}
              {engineError && (
                <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-black/80 p-6 text-center" role="alert">
                  <p className="text-[13px] font-medium text-danger">Preview is unavailable</p>
                  <p className="text-[12px] text-muted">{engineError}</p>
                </div>
              )}
            </div>
            {/* Outside the clipped frame so handles of a layer larger than the canvas stay reachable. */}
            {!empty && !engineError && canvas && fit.w > 0 && <LayerOverlay width={fit.w} height={fit.h} bleed={{ x: (box.w - fit.w) / 2, y: (box.h - fit.h) / 2 }} />}
          </div>
        </div>
      </div>
      <div className="grid h-12 shrink-0 grid-cols-[1fr_auto_1fr] items-center gap-2 px-3">
        <span className="tabular text-[12px]">
          <span className="text-fg">{formatTime(timeUs)}</span>
          <span className="text-muted"> / {formatTime(duration)}</span>
        </span>
        <div className="flex items-center gap-2">
          <IconButton label="Go to start (Home)" onClick={() => seek(0)} disabled={empty}>
            <SkipBack size={16} />
          </IconButton>
          <button
            type="button"
            aria-label={playing ? "Pause (Space)" : "Play (Space)"}
            title={playing ? "Pause (Space)" : "Play (Space)"}
            disabled={empty}
            onClick={togglePlay}
            className="flex h-10 w-10 items-center justify-center rounded-full bg-fg text-black transition-transform duration-[120ms] ease-out hover:scale-105 active:scale-95 disabled:cursor-not-allowed disabled:opacity-30"
          >
            {playing ? <Pause size={18} fill="currentColor" /> : <Play size={18} fill="currentColor" className="translate-x-px" />}
          </button>
          <IconButton label="Go to end (End)" onClick={() => seek(duration)} disabled={empty}>
            <SkipForward size={16} />
          </IconButton>
        </div>
        <div className="flex items-center justify-end gap-1">
          <IconButton
            label={area ? (showSafe ? "Hide the Reels and TikTok safe zone" : "Show the Reels and TikTok safe zone") : "The safe zone applies to vertical videos"}
            active={showSafe && !!area}
            disabled={!area}
            onClick={toggleSafe}
          >
            <Smartphone size={16} />
          </IconButton>
          {canvas && <RatioMenu />}
        </div>
      </div>
    </section>
  );
}
