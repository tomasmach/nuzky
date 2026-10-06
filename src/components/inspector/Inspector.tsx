import { useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { AudioLines, Blend, Captions, Film, Image as ImageIcon, Type } from "lucide-react";
import { TRANSITIONS } from "../../lib/presets";
import { findClip, isCaptionTrack, mainCuts, useEditor } from "../../lib/store";
import { formatDuration, formatTime } from "../../lib/time";
import type { Asset, Clip } from "../../lib/types";
import { TransitionSettings } from "../panel/TransitionsTab";
import { TabBar } from "../ui";
import { AnimationSection } from "./AnimationSection";
import { AdjustSection, AudioSection, SpeedSection } from "./MediaSections";
import { MultiInspector } from "./MultiInspector";
import { ProjectSection } from "./ProjectSection";
import { TextSection } from "./TextSection";
import { TransformSection } from "./TransformSection";

type TabId = "video" | "adjust" | "speed" | "animation" | "audio" | "text" | "transform";
type Kind = "video" | "image" | "audio" | "text" | "caption";

function tabsFor(kind: Kind, hasSound: boolean): { id: TabId; label: string }[] {
  if (kind === "audio") return [{ id: "audio", label: "Audio" }];
  if (kind === "text" || kind === "caption")
    return [
      { id: "text", label: "Text" },
      { id: "animation", label: "Animation" },
      { id: "transform", label: "Transform" },
    ];
  const tabs: { id: TabId; label: string }[] = [
    { id: "video", label: kind === "image" ? "Image" : "Video" },
    { id: "adjust", label: "Adjust" },
  ];
  if (kind === "video") tabs.push({ id: "speed", label: "Speed" });
  tabs.push({ id: "animation", label: "Animation" });
  if (hasSound) tabs.push({ id: "audio", label: "Audio" });
  return tabs;
}

const KIND_ICON = { video: Film, image: ImageIcon, audio: AudioLines, text: Type, caption: Captions };

function Header({ icon: Icon, title, detail }: { icon: typeof Film; title: string; detail: string }) {
  return (
    <div className="shrink-0 border-b border-line px-4 py-3">
      <div className="flex items-center gap-1.5 text-[13px] font-medium text-fg">
        <Icon size={14} className="shrink-0 text-muted" />
        <span className="truncate">{title}</span>
      </div>
      <div className="tabular mt-0.5 text-[12px] text-muted">{detail}</div>
    </div>
  );
}

type Chosen = Partial<Record<Kind, TabId>>;

function ClipInspector({ clip, kind, asset, chosen, onChoose }: { clip: Clip; kind: Kind; asset?: Asset; chosen: Chosen; onChoose: (c: Chosen) => void }) {
  const c = clip.content;
  const hasSound = kind === "video" && !!asset?.hasAudio;
  const tabs = tabsFor(kind, hasSound);
  const tab = tabs.find((t) => t.id === chosen[kind])?.id ?? tabs[0].id;
  const title = kind === "caption" ? "Caption" : kind === "text" ? "Text" : (asset?.name ?? "Missing media");
  const detail =
    formatDuration(clip.durationUs) +
    (asset && asset.kind !== "image" && c.type === "media" ? ` · from ${formatDuration(c.sourceInUs)} of ${formatDuration(asset.durationUs)}` : "");

  return (
    <>
      <Header icon={KIND_ICON[kind]} title={title} detail={detail} />
      <TabBar label="Clip settings" tabs={tabs} value={tab} onChange={(id) => onChoose({ ...chosen, [kind]: id })} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {(tab === "video" || tab === "transform") && <TransformSection clip={clip} asset={asset} />}
        {tab === "text" && c.type === "text" && <TextSection clip={clip} text={c.text} style={c.style} caption={kind === "caption"} />}
        {tab === "adjust" && c.type === "media" && <AdjustSection clip={clip} content={c} />}
        {tab === "speed" && c.type === "media" && <SpeedSection clip={clip} content={c} asset={asset} />}
        {tab === "animation" && <AnimationSection clip={clip} />}
        {tab === "audio" && c.type === "media" && (
          <>
            <AudioSection clip={clip} content={c} />
            {kind === "audio" && <SpeedSection clip={clip} content={c} asset={asset} />}
          </>
        )}
      </div>
    </>
  );
}

function CutInspector({ clipId }: { clipId: string }) {
  const cut = useEditor(useShallow((s) => (s.snap ? mainCuts(s.snap.project).find((c) => c.clipId === clipId) : undefined)));
  if (!cut) return null;
  const t = cut.transition;
  return (
    <>
      {/* The icon says what is selected, like for clips; a kind icon such as Slide left's arrow would read as "back". */}
      <Header
        icon={Blend}
        title={t ? (TRANSITIONS.find((k) => k.kind === t.kind)?.label ?? "Transition") : "Cut"}
        detail={`${t ? "Transition · " : ""}cut at ${formatTime(cut.atUs)}`}
      />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <TransitionSettings cut={cut} />
      </div>
    </>
  );
}

export function Inspector() {
  const project = useEditor((s) => s.snap?.project);
  const selection = useEditor((s) => s.selection);
  const cut = useEditor((s) => s.cut);
  // Remembered per clip kind, so picking another clip keeps the tab you were on, like CapCut.
  const [chosen, setChosen] = useState<Chosen>({});
  const found = project && selection.length === 1 ? findClip(project, selection[0]) : null;

  let body;
  if (!project) body = null;
  else if (cut) body = <CutInspector clipId={cut} />;
  else if (selection.length > 1) body = <MultiInspector ids={selection} />;
  else if (!found) body = <ProjectSection />;
  else {
    const { clip, track } = found;
    const c = clip.content;
    const asset = c.type === "media" ? project.assets.find((a) => a.id === c.assetId) : undefined;
    const kind: Kind = c.type === "text" ? (isCaptionTrack(track) ? "caption" : "text") : track.kind === "audio" ? "audio" : (asset?.kind ?? "video");
    body = <ClipInspector key={clip.id} clip={clip} kind={kind} asset={asset} chosen={chosen} onChoose={setChosen} />;
  }

  return (
    <aside className="flex w-[300px] shrink-0 flex-col overflow-hidden border-l border-line bg-panel" aria-label="Inspector">
      {body}
    </aside>
  );
}
