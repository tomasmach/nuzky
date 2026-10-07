import { Blend, Captions, Film, Music, Palette, ScrollText, Type } from "lucide-react";
import { useEditor, type PanelTab } from "../../lib/store";
import { AiLock, TabPanel, tabIds, tabListKeys } from "../ui";
import { AudioTab } from "./AudioTab";
import { CaptionsTab } from "./CaptionsTab";
import { FiltersTab } from "./FiltersTab";
import { MediaTab } from "./MediaTab";
import { TextTab } from "./TextTab";
import { TranscriptTab } from "./TranscriptTab";
import { TransitionsTab } from "./TransitionsTab";

// CapCut's order.
const TABS: { id: PanelTab; label: string; icon: typeof Film }[] = [
  { id: "media", label: "Media", icon: Film },
  { id: "audio", label: "Audio", icon: Music },
  { id: "text", label: "Text", icon: Type },
  { id: "captions", label: "Captions", icon: Captions },
  { id: "transcript", label: "Transcript", icon: ScrollText },
  { id: "transitions", label: "Transitions", icon: Blend },
  { id: "filters", label: "Filters", icon: Palette },
];

const TAB_IDS = TABS.map((t) => t.id);

export function LeftPanel() {
  const tab = useEditor((s) => s.panelTab);
  const setTab = (id: PanelTab) => useEditor.setState({ panelTab: id });
  return (
    <aside className="flex w-[340px] shrink-0 flex-col border-r border-line bg-panel">
      {/* Tabs size to their labels and share the leftover width, so "Captions" and "Transitions" never touch. */}
      <div className="flex shrink-0 gap-1 border-b border-line px-1" role="tablist" aria-label="Library" onKeyDown={tabListKeys(TAB_IDS, tab, setTab)}>
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            id={tabIds("library", t.id).tab}
            data-tab={t.id}
            aria-selected={tab === t.id}
            aria-controls={tab === t.id ? tabIds("library", t.id).panel : undefined}
            tabIndex={tab === t.id ? 0 : -1}
            onClick={() => setTab(t.id)}
            className={`flex flex-auto flex-col items-center gap-0.5 px-1 py-2 text-[11px] whitespace-nowrap transition-colors duration-[120ms] ${
              tab === t.id ? "text-accent shadow-[inset_0_-2px_0_var(--color-accent)]" : "text-muted hover:text-fg"
            }`}
          >
            <t.icon size={17} />
            <span>{t.label}</span>
          </button>
        ))}
      </div>
      <TabPanel group="library" id={tab} className="flex min-h-0 flex-1 flex-col">
        <AiLock>
          {tab === "media" && <MediaTab />}
          {tab === "audio" && <AudioTab />}
          {tab === "text" && <TextTab />}
          {tab === "captions" && <CaptionsTab />}
          {tab === "transcript" && <TranscriptTab />}
          {tab === "transitions" && <TransitionsTab />}
          {tab === "filters" && <FiltersTab />}
        </AiLock>
      </TabPanel>
    </aside>
  );
}
