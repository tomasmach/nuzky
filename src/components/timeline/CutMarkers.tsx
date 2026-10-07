import { memo } from "react";
import { Plus } from "lucide-react";
import { DEFAULT_TRANSITION_US, TRANSITIONS } from "../../lib/presets";
import { mainCuts, useEditor } from "../../lib/store";
import { US, formatDuration } from "../../lib/time";
import type { Project } from "../../lib/types";
import { TRANSITION_ICONS } from "../panel/TransitionsTab";

/** A square on every main-track cut: "+" adds a Dissolve, an icon opens the existing transition. */
export const CutMarkers = memo(function CutMarkers({ project, zoom, locked }: { project: Project; zoom: number; locked: boolean }) {
  const selected = useEditor((s) => s.cut);
  const { edit, selectCut } = useEditor.getState();
  return (
    <>
      {mainCuts(project).map((cut) => {
        const x = (cut.atUs / US) * zoom;
        const t = cut.transition;
        const Icon = t ? TRANSITION_ICONS[t.kind] : Plus;
        const name = t ? TRANSITIONS.find((k) => k.kind === t.kind)?.label : null;
        const on = selected === cut.clipId;
        return (
          <div key={cut.clipId}>
            {t && (
              <div
                className={`pointer-events-none absolute inset-y-1 z-[15] rounded-sm bg-black/45 ${on ? "border-2 border-accent" : "border border-fg/70"}`}
                style={{ left: x - ((t.durationUs / US) * zoom) / 2, width: (t.durationUs / US) * zoom }}
              />
            )}
            <button
              type="button"
              aria-label={t ? `${name} transition, ${formatDuration(t.durationUs)}` : "Add transition"}
              title={t ? `${name} · ${formatDuration(t.durationUs)}` : locked ? "Add transition: the AI is editing" : "Add transition (Dissolve)"}
              aria-pressed={on}
              aria-disabled={(locked && !t) || undefined}
              onPointerDown={(e) => e.stopPropagation()}
              onClick={async () => {
                if (locked && !t) return;
                if (!t && !(await edit({ type: "setTransition", clipId: cut.clipId, transition: { kind: "dissolve", durationUs: DEFAULT_TRANSITION_US } }))) return;
                selectCut(cut.clipId);
              }}
              className={`absolute top-1/2 z-20 flex h-[18px] w-[18px] -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-[4px] border transition-colors duration-[120ms] ease-out aria-disabled:cursor-not-allowed ${
                t
                  ? on
                    ? "border-accent bg-accent text-black"
                    : "border-black/40 bg-fg text-black hover:bg-white"
                  : "border-fg/40 bg-panel/90 text-transparent hover:border-fg hover:bg-fg hover:text-black focus-visible:text-fg"
              }`}
              style={{ left: x }}
            >
              <Icon size={12} strokeWidth={2.5} />
            </button>
          </div>
        );
      })}
    </>
  );
});
