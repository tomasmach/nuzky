"use client";

import { useEffect, useRef, useState } from "react";
import { Captions, Plus, Scissors, Sparkles, Upload } from "lucide-react";
import { AgentPanel } from "./AgentPanel";
import { EditorMock, type EditorFocus } from "./EditorMock";
import { Fit } from "./Fit";
import { Waves } from "./Waves";

// `side` places the tooltip where it covers nothing the tab is showing. Without it the
// tooltip goes below the hotspot, or above it in the lower half.
type Spot = { x: number; y: number; title: string; text: string; side?: "left" | "right" | "above" };
type Tab = { id: EditorFocus; label: string; icon: typeof Scissors; summary: string; spots: Spot[] };

// Hotspot positions are in the editor's own 1200 × 740 space.
const tabs: Tab[] = [
  {
    id: "edit",
    label: "Edit",
    icon: Scissors,
    summary: "A timeline, a live preview and an inspector, laid out the way you already know.",
    spots: [
      { x: 500, y: 656, title: "Magnetic timeline", text: "Clips snap together and every cut lands on the exact frame, even in phone video." },
      { x: 306, y: 250, title: "Your media", text: "Drop in video, sound and images. Phone footage opens the right way up." },
      { x: 934, y: 262, title: "Inspector", text: "Scale, opacity, volume and speed, each with a value you can type." },
    ],
  },
  {
    id: "captions",
    label: "Captions",
    icon: Captions,
    summary: "Whisper writes the captions on your computer. Pick a style, fix a word, done.",
    spots: [
      { x: 724, y: 330, title: "Captions in one click", text: "Transcribed on your computer, with the spoken word lit as it plays.", side: "above" },
      { x: 236, y: 573, title: "A clip per line", text: "Fix a word or move a line right on the timeline." },
      { x: 934, y: 452, title: "Caption styles", text: "Bold, Clean or Pop, or make your own." },
    ],
  },
  {
    id: "agent",
    label: "AI agent",
    icon: Sparkles,
    summary: "Claude Code or Codex edits live in the open app. One undo takes the whole run back.",
    spots: [
      { x: 854, y: 130, title: "Your agent, in the app", text: "Connect Claude Code or Codex in one click, or any MCP client.", side: "left" },
      { x: 854, y: 398, title: "One undo", text: "Every change the agent made goes back in one step.", side: "left" },
    ],
  },
  {
    id: "export",
    label: "Export",
    icon: Upload,
    summary: "One click for Reels and TikTok. The export looks exactly like the preview.",
    spots: [
      { x: 380, y: 300, title: "Same renderer", text: "The export is drawn by the renderer you watched in the preview.", side: "left" },
      { x: 820, y: 360, title: "Made for Reels and TikTok", text: "1080 × 1920 at 30 fps, with the sound levelled to −14 LUFS.", side: "right" },
    ],
  },
];

const ADVANCE_MS = 7000;
const TIP_W = 292;

function tipPosition(s: Spot): React.CSSProperties {
  if (s.side === "left") return { left: s.x - 30 - TIP_W, top: s.y - 24 };
  if (s.side === "right") return { left: s.x + 30, top: s.y - 24 };
  const left = Math.min(Math.max(s.x - TIP_W / 2, 16), 1200 - TIP_W - 16);
  return s.side === "above" || s.y > 420 ? { left, bottom: 740 - s.y + 30 } : { left, top: s.y + 30 };
}

function ExportDialog() {
  const rows = [
    ["Size", "1080 × 1920"],
    ["Frame rate", "30 fps"],
    ["Loudness", "−14 LUFS · −1 dBTP"],
    ["Format", "MP4 · H.264 + AAC"],
    ["Save to", "Movies/morning-run.mp4"],
  ];
  return (
    <div className="absolute inset-0 flex items-center justify-center rounded-[14px] bg-black/45 animate-[fade-up_200ms_ease-out]">
      <div className="flex w-[440px] flex-col gap-[18px] rounded-[20px] bg-[#232327] p-6 text-left shadow-[0_24px_60px_rgb(0_0_0/0.6)] ring-1 ring-white/10">
        <span className="text-[17px] font-semibold">Export</span>
        <div className="flex rounded-[9px] bg-white/[0.045] p-0.5">
          {["Reels & TikTok", "Original", "Custom"].map((t, i) => (
            <span key={t} className={`flex-1 rounded-[7px] py-1.5 text-center text-[12px] font-medium ${i === 0 ? "bg-white/15" : "text-muted"}`}>
              {t}
            </span>
          ))}
        </div>
        <div className="flex flex-col gap-2.5">
          {rows.map(([a, b]) => (
            <div key={a} className="flex items-center gap-4 text-[13px]">
              <span className="w-24 text-right text-muted">{a}</span>
              <span className="tabular-nums">{b}</span>
            </div>
          ))}
        </div>
        <div className="flex justify-end gap-2.5 pt-1.5">
          <span className="flex h-8 items-center rounded-full bg-white/[0.09] px-4 text-[13px] font-medium ring-1 ring-inset ring-white/[0.07]">Cancel</span>
          <span className="flex h-8 items-center rounded-full bg-accent-strong px-4 text-[13px] font-medium text-white">Export</span>
        </div>
      </div>
    </div>
  );
}

export function Showcase() {
  const [active, setActive] = useState(0);
  const [spot, setSpot] = useState(0);
  const [auto, setAuto] = useState(true);
  const [paused, setPaused] = useState(false);
  const [inView, setInView] = useState(false);
  const section = useRef<HTMLElement>(null);
  const tab = tabs[active];

  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) setAuto(false);
    // `?tour=captions` opens the tour on a tab, for links that point at one feature.
    const linked = tabs.findIndex((t) => t.id === new URLSearchParams(window.location.search).get("tour"));
    if (linked >= 0) {
      setActive(linked);
      setAuto(false);
    }
    const io = new IntersectionObserver(([e]) => setInView(e.isIntersecting), { threshold: 0.35 });
    if (section.current) io.observe(section.current);
    return () => io.disconnect();
  }, []);

  const running = auto && !paused && inView;
  useEffect(() => {
    if (!running) return;
    const id = window.setTimeout(() => {
      setActive((a) => (a + 1) % tabs.length);
      setSpot(0);
    }, ADVANCE_MS);
    return () => window.clearTimeout(id);
  }, [running, active]);

  const choose = (i: number) => {
    setAuto(false);
    setActive(i);
    setSpot(0);
  };

  const current = tab.spots[spot];

  return (
    <section id="editor" ref={section} className="relative overflow-hidden pb-10 pt-28 sm:pt-[150px]">
      <Waves height={0.42} intensity={1.25} />
      <div className="relative z-10 mx-auto flex max-w-[1240px] flex-col items-center gap-7 px-5 text-center">
        <h2 className="heading-lg">
          Everything where you expect it.
          <br />
          <span className="text-subtle">And an agent that edits with you.</span>
        </h2>

        <div role="tablist" aria-label="Editor tour" className="glass flex h-12 max-w-full items-center gap-0.5 overflow-x-auto rounded-full p-[5px] no-scrollbar">
          {tabs.map((t, i) => (
            <button
              key={t.id}
              role="tab"
              id={`tab-${t.id}`}
              aria-selected={i === active}
              aria-controls="editor-stage"
              onClick={() => choose(i)}
              className={`flex h-[38px] shrink-0 items-center gap-2 rounded-full px-3.5 text-[14px] font-medium transition-colors sm:px-[18px] ${
                i === active ? "bg-white/[0.16] text-fg" : "text-muted hover:text-fg"
              }`}
            >
              <t.icon className="hidden size-[15px] sm:block" />
              {t.label}
            </button>
          ))}
        </div>

        <div
          id="editor-stage"
          role="tabpanel"
          aria-labelledby={`tab-${tab.id}`}
          className="w-full"
          onPointerEnter={() => setPaused(true)}
          onPointerLeave={() => setPaused(false)}
          onFocus={() => setPaused(true)}
          onBlur={() => setPaused(false)}
        >
          <Fit width={1200} height={740} className="mx-auto">
            <div className="relative size-full rounded-[14px] shadow-[0_50px_140px_rgb(0_0_0/0.9),0_0_0_1px_rgb(255_255_255/0.08)]">
              <EditorMock focus={tab.id} />
              {tab.id === "agent" && (
                <div className="absolute bottom-1.5 right-1.5 top-[54px] w-[340px] animate-[fade-up_240ms_ease-out] shadow-[0_30px_60px_rgb(0_0_0/0.55)]">
                  <AgentPanel compact className="h-full ring-white/[0.12]" />
                </div>
              )}
              {tab.id === "export" && <ExportDialog />}

              {tab.spots.map((s, i) => (
                <button
                  key={`${tab.id}-${i}`}
                  aria-label={s.title}
                  aria-pressed={i === spot}
                  onPointerEnter={() => setSpot(i)}
                  onFocus={() => setSpot(i)}
                  onClick={() => setSpot(i)}
                  className="group absolute hidden size-11 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full md:flex"
                  style={{ left: s.x, top: s.y }}
                >
                  {i === spot && <span className="absolute inset-0 rounded-full border-[1.5px] border-white/40 animate-[pulse-ring_1.8s_ease-out_infinite]" />}
                  <span
                    className={`flex size-[30px] items-center justify-center rounded-full shadow-[0_4px_14px_rgb(0_0_0/0.5)] transition-colors ${
                      i === spot ? "bg-fg text-bg" : "glass text-fg group-hover:bg-white/20"
                    }`}
                  >
                    <Plus className="size-[15px]" />
                  </span>
                </button>
              ))}

              {current && (
                <div
                  key={`${tab.id}-${spot}`}
                  role="tooltip"
                  className="glass pointer-events-none absolute hidden w-[292px] flex-col gap-1.5 rounded-[20px] bg-[#1c1c1f]/80 px-[18px] py-4 text-left animate-[fade-up_180ms_ease-out] md:flex"
                  style={tipPosition(current)}
                >
                  <span className="text-[15px] font-semibold">{current.title}</span>
                  <span className="text-[13px] leading-[1.45] text-fg/75">{current.text}</span>
                </div>
              )}
            </div>
          </Fit>
        </div>

        <div className="flex flex-col items-center gap-4 pt-3">
          <p key={tab.id} className="min-h-[1.5em] text-[17px] text-muted animate-[fade-up_240ms_ease-out]">
            {tab.summary}
          </p>
          <div className="flex items-center gap-1.5" aria-hidden>
            {tabs.map((t, i) => (
              <span key={t.id} className={`relative h-1.5 overflow-hidden rounded-full bg-white/25 transition-[width] duration-300 ${i === active ? "w-7" : "w-1.5"}`}>
                {i === active && (
                  <span
                    key={`${active}-${running}`}
                    className="absolute inset-0 origin-left rounded-full bg-fg"
                    style={running ? { animation: `progress ${ADVANCE_MS}ms linear forwards` } : undefined}
                  />
                )}
              </span>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
