import type { KeyboardEvent } from "react";
import { Blend, Captions, Film, Music, Palette, Type } from "lucide-react";
import { useEditor, type PanelTab } from "../../lib/store";
import { AudioTab } from "./AudioTab";
import { CaptionsTab } from "./CaptionsTab";
import { FiltersTab } from "./FiltersTab";
import { MediaTab } from "./MediaTab";
import { TextTab } from "./TextTab";
import { TransitionsTab } from "./TransitionsTab";

// CapCut's order.
const TABS: { id: PanelTab; label: string; icon: typeof Film }[] = [
  { id: "media", label: "Media", icon: Film },
  { id: "audio", label: "Audio", icon: Music },
  { id: "text", label: "Text", icon: Type },
  { id: "captions", label: "Captions", icon: Captions },
  { id: "transitions", label: "Transitions", icon: Blend },
  { id: "filters", label: "Filters", icon: Palette },
];

export function LeftPanel() {
  const tab = useEditor((s) => s.panelTab);
  const setTab = (id: PanelTab) => useEditor.setState({ panelTab: id });
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    const i = TABS.findIndex((t) => t.id === tab);
    const next = TABS[(i + (e.key === "ArrowRight" ? 1 : TABS.length - 1)) % TABS.length];
    setTab(next.id);
    (e.currentTarget.querySelector(`[data-tab="${next.id}"]`) as HTMLElement | null)?.focus();
  };
  return (
    <aside className="flex w-[340px] shrink-0 flex-col border-r border-line bg-panel">
      <div className="flex shrink-0 border-b border-line" role="tablist" aria-label="Library" onKeyDown={onKeyDown}>
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            data-tab={t.id}
            aria-selected={tab === t.id}
            tabIndex={tab === t.id ? 0 : -1}
            onClick={() => setTab(t.id)}
            className={`flex min-w-0 flex-1 flex-col items-center gap-0.5 py-2 text-[11px] transition-colors duration-[120ms] ${
              tab === t.id ? "text-accent shadow-[inset_0_-2px_0_var(--color-accent)]" : "text-muted hover:text-fg"
            }`}
          >
            <t.icon size={17} />
            <span className="max-w-full truncate">{t.label}</span>
          </button>
        ))}
      </div>
      {tab === "media" && <MediaTab />}
      {tab === "audio" && <AudioTab />}
      {tab === "text" && <TextTab />}
      {tab === "captions" && <CaptionsTab />}
      {tab === "transitions" && <TransitionsTab />}
      {tab === "filters" && <FiltersTab />}
    </aside>
  );
}
