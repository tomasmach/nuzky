import { useState, type CSSProperties } from "react";
import { useShallow } from "zustand/react/shallow";
import { ArrowLeft, ArrowUp, Ban, Blend, ChevronsLeft, Droplet, Moon, Sun, ZoomIn, type LucideIcon } from "lucide-react";
import { DEFAULT_TRANSITION_US, MAX_TRANSITION_US, TRANSITIONS } from "../../lib/presets";
import { mainClips, transitionTarget, useEditor, type Cut } from "../../lib/store";
import { US, formatTime } from "../../lib/time";
import type { TransitionKind } from "../../lib/types";
import { PresetTile, Slider } from "../ui";

export const TRANSITION_ICONS: Record<TransitionKind, LucideIcon> = {
  dissolve: Blend,
  fadeBlack: Moon,
  fadeWhite: Sun,
  slideLeft: ArrowLeft,
  slideUp: ArrowUp,
  zoomIn: ZoomIn,
  wipeLeft: ChevronsLeft,
  blur: Droplet,
};

/** Animation names per layer, defined in index.css. */
const MOTION: Record<TransitionKind, { a?: string; b: string; flash?: string }> = {
  dissolve: { b: "t-dissolve-b" },
  fadeBlack: { b: "t-swap-b", flash: "#000" },
  fadeWhite: { b: "t-swap-b", flash: "#fff" },
  slideLeft: { a: "t-slide-left-a", b: "t-slide-left-b" },
  slideUp: { a: "t-slide-up-a", b: "t-slide-up-b" },
  zoomIn: { a: "t-zoom-a", b: "t-zoom-b" },
  wipeLeft: { b: "t-wipe-b" },
  blur: { a: "t-blur-a", b: "t-blur-b" },
};

function layer(url: string | null | undefined, fallback: string): CSSProperties {
  return url ? { backgroundImage: `url(${url})`, backgroundSize: "cover", backgroundPosition: "center" } : { background: fallback };
}

/** The two clips around the cut, playing the transition between them. */
function TransitionPreview({ kind, a, b }: { kind: TransitionKind; a?: string | null; b?: string | null }) {
  const m = MOTION[kind];
  return (
    <span className="absolute inset-0 overflow-hidden">
      <span className={`absolute inset-0 ${m.a ? "pv" : ""}`} style={{ ...layer(a, "#25435a"), animationName: m.a }} />
      <span className="pv absolute inset-0" style={{ ...layer(b, "#5a4520"), animationName: m.b }} />
      {m.flash && <span className="pv absolute inset-0" style={{ background: m.flash, animationName: "t-flash" }} />}
    </span>
  );
}

function useCutThumbs(cut: Cut | null) {
  return useEditor((s) => {
    const clips = s.snap ? mainClips(s.snap.project) : [];
    const i = cut ? clips.findIndex((c) => c.id === cut.clipId) : -1;
    const thumb = (k: number) => {
      const c = clips[k]?.content;
      return c?.type === "media" ? s.thumbs[c.assetId] : null;
    };
    return i > 0 ? `${thumb(i - 1) ?? ""}|${thumb(i) ?? ""}` : "|";
  }).split("|");
}

/** Kind grid and duration for one cut; used by the Transitions tab and the inspector. */
export function TransitionEditor({ cut }: { cut: Cut }) {
  const edit = useEditor((s) => s.edit);
  const selectCut = useEditor((s) => s.selectCut);
  const shortest = useEditor((s) => {
    const clips = s.snap ? mainClips(s.snap.project) : [];
    const i = clips.findIndex((c) => c.id === cut.clipId);
    return i > 0 ? Math.min(clips[i - 1].durationUs, clips[i].durationUs) : MAX_TRANSITION_US;
  });
  const [defaultUs, setDefaultUs] = useState(DEFAULT_TRANSITION_US);
  const [a, b] = useCutThumbs(cut);
  const maxUs = Math.max(100_000, Math.min(MAX_TRANSITION_US, shortest));
  const durationUs = cut.transition?.durationUs ?? Math.min(defaultUs, maxUs);

  const apply = (kind: TransitionKind | null) => {
    edit({ type: "setTransition", clipId: cut.clipId, transition: kind ? { kind, durationUs } : null });
    selectCut(kind ? cut.clipId : null);
  };
  const setDuration = (s: number) => {
    const us = Math.round(s * US);
    if (cut.transition) edit({ type: "setTransition", clipId: cut.clipId, transition: { ...cut.transition, durationUs: us } }, `${cut.clipId}:transition-duration`);
    else setDefaultUs(us);
  };

  return (
    <div className="flex flex-col gap-3">
      <div className="grid grid-cols-3 gap-2">
        <PresetTile label="None" selected={!cut.transition} onClick={() => apply(null)} title="No transition (hard cut)">
          <span className="absolute inset-0 flex items-center justify-center text-muted">
            <Ban size={18} />
          </span>
        </PresetTile>
        {TRANSITIONS.map((t) => (
          <PresetTile key={t.kind} label={t.label} selected={cut.transition?.kind === t.kind} onClick={() => apply(t.kind)}>
            <TransitionPreview kind={t.kind} a={a || null} b={b || null} />
          </PresetTile>
        ))}
      </div>
      <Slider
        label="Duration"
        value={durationUs / US}
        min={0.1}
        max={maxUs / US}
        step={0.1}
        unit="s"
        format={(v) => v.toFixed(1)}
        onChange={setDuration}
        title={cut.transition ? undefined : "Duration for the transition you pick next"}
      />
    </div>
  );
}

export function TransitionsTab() {
  const target = useEditor(useShallow((s) => (s.snap ? transitionTarget(s.snap.project, s.selection, s.cut, s.timeUs) : null)));
  if (!target)
    return (
      <div className="m-3 flex flex-1 flex-col items-center justify-center gap-2 rounded-lg border border-dashed border-line p-6 text-center">
        <Blend size={28} className="text-muted" />
        <p className="text-[13px] text-fg">No cuts yet</p>
        <p className="text-[12px] text-muted">Put two clips on the main track to add a transition between them.</p>
      </div>
    );
  return (
    <div className="flex flex-col gap-3 overflow-y-auto p-3">
      <p className="tabular text-[12px] text-muted">
        Cut at <span className="text-fg">{formatTime(target.atUs)}</span>
      </p>
      <TransitionEditor cut={target} />
    </div>
  );
}
