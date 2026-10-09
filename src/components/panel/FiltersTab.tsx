import { useShallow } from "zustand/react/shallow";
import { Palette } from "lucide-react";
import { FILTERS, adjustCss, sameAdjust } from "../../lib/presets";
import { findClip, useEditor } from "../../lib/store";
import type { Adjust } from "../../lib/types";
import { PresetTile } from "../ui";

// Shown when the clip has no thumbnail yet: a sky, a horizon and grass reveal colour shifts.
const SCENE = "linear-gradient(180deg, #7fb2e5 0%, #d9c7a3 52%, #4f7d3f 53%, #2c4a25 100%)";

function FilterPreview({ adjust, thumb }: { adjust: Adjust; thumb: string | null }) {
  const css = adjustCss(adjust);
  return (
    <span className="absolute inset-0" style={{ filter: css.filter }}>
      <span className="absolute inset-0" style={thumb ? { backgroundImage: `url(${thumb})`, backgroundSize: "cover", backgroundPosition: "center" } : { background: SCENE }} />
      {css.tint && <span className="absolute inset-0 mix-blend-soft-light" style={{ background: css.tint }} />}
      {css.vignette > 0 && <span className="absolute inset-0" style={{ background: `radial-gradient(ellipse at center, transparent 45%, rgba(0,0,0,${css.vignette}) 100%)` }} />}
    </span>
  );
}

/** The single selected video or image clip, if any. */
function useFilterTarget() {
  return useEditor(
    useShallow((s) => {
      if (!s.snap || s.selection.length !== 1) return { clipId: null, adjust: null, thumb: null };
      const found = findClip(s.snap.project, s.selection[0]);
      const c = found?.clip.content;
      const asset = c?.type === "media" ? s.snap.project.assets.find((a) => a.id === c.assetId) : undefined;
      if (!found || c?.type !== "media" || !asset || asset.kind === "audio") return { clipId: null, adjust: null, thumb: null };
      return { clipId: found.clip.id, adjust: c.adjust, thumb: s.thumbs[asset.id] ?? null };
    }),
  );
}

export function FiltersTab() {
  const { clipId, adjust, thumb } = useFilterTarget();
  const edit = useEditor((s) => s.edit);
  return (
    <div className="flex flex-col gap-3 overflow-y-auto p-3.5">
      {!clipId && (
        <p className="flex items-center gap-1.5 text-[12px] text-muted">
          <Palette size={14} className="shrink-0" /> Select a video or image clip to apply a filter.
        </p>
      )}
      <div className="grid grid-cols-[repeat(auto-fill,minmax(84px,1fr))] gap-2">
        {FILTERS.map((f) => (
          <PresetTile
            key={f.id}
            label={f.label}
            selected={!!adjust && sameAdjust(adjust, f.adjust)}
            disabled={!clipId}
            title={clipId ? `Apply ${f.label}` : "Select a video or image clip first"}
            onClick={() => clipId && edit({ type: "updateClip", clipId, adjust: f.adjust })}
          >
            <FilterPreview adjust={f.adjust} thumb={thumb} />
          </PresetTile>
        ))}
      </div>
      {clipId && adjust && !FILTERS.some((f) => sameAdjust(adjust, f.adjust)) && <p className="text-[12px] text-muted">This clip uses custom adjustments. Picking a filter replaces them.</p>}
    </div>
  );
}
