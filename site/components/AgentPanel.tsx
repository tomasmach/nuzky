import { ArrowUp, Check, ChevronDown, Loader, SquarePen, Undo2, X } from "lucide-react";

export type Exchange = {
  ask: string;
  reply: string;
  steps: [string, string][];
  result: string;
  changes: [string, string][];
  /** An export cannot be taken back, so its result has no Undo. */
  undo?: boolean;
};

export const exchanges: Exchange[] = [
  {
    ask: "Cut the ums and long pauses, then add captions in the Bold style.",
    reply: "On it. I'll transcribe first, then cut.",
    steps: [
      ["Transcribe 0:31 of audio", "12 s"],
      ["Remove 14 filler words", "0.4 s"],
      ["Cut 6 pauses over 0.8 s", "0.3 s"],
      ["Add captions · Bold", "0.2 s"],
    ],
    result: "21 changes",
    changes: [
      ["Cut “um” at", "00:09.4"],
      ["Cut pause at", "00:12.1"],
      ["Added 9 captions", "00:00–00:31"],
    ],
  },
  {
    ask: "The intro drags. Start on the first thing I say.",
    reply: "Trimming the quiet start of the first clip.",
    steps: [
      ["Find the first spoken word", "0.2 s"],
      ["Trim 2.4 s from the start", "0.1 s"],
    ],
    result: "1 change",
    changes: [["Trimmed the start to", "00:02.4"]],
  },
  {
    ask: "Put Sunrise under the whole video, quietly.",
    reply: "Adding it under everything at −18 dB, with a fade at the end.",
    steps: [
      ["Import Sunrise.m4a", "0.3 s"],
      ["Set the volume to −18 dB", "0.1 s"],
      ["Fade out over 2 s", "0.1 s"],
    ],
    result: "3 changes",
    changes: [
      ["Added music at", "00:00"],
      ["Fade out from", "00:27.0"],
    ],
  },
  {
    ask: "The caption at 0:07 should say “ridge”, not “bridge”.",
    reply: "Fixing the word in the transcript.",
    steps: [
      ["Correct “bridge” to “ridge”", "0.1 s"],
      ["Rebuild 1 caption", "0.1 s"],
    ],
    result: "1 change",
    changes: [["Fixed the caption at", "00:07.2"]],
  },
  {
    ask: "Looks good. Export it for Reels.",
    reply: "Exporting 1080 × 1920 at 30 fps.",
    steps: [
      ["Level the sound to −14 LUFS", "1.1 s"],
      ["Render 0:29 of video", "9 s"],
      ["Save morning-run.mp4", "0.1 s"],
    ],
    result: "Exported · 12.4 MB",
    changes: [["Saved to", "Movies/morning-run.mp4"]],
    undo: false,
  },
];

/** Stages of one exchange: 0 asked, 1 replied, 2 + k running step k, `doneStage` finished. */
export const doneStage = (x: Exchange) => 2 + x.steps.length;

const appear = "animate-[fade-up_240ms_ease-out]";

export function Run({ x, stage, latest = true }: { x: Exchange; stage: number; latest?: boolean }) {
  const running = stage - 2;
  return (
    <>
      <div className={`flex shrink-0 justify-end ${appear}`}>
        <p className="max-w-[290px] rounded-[14px] rounded-br-[4px] bg-raised px-3.5 py-2.5 text-[13px] leading-[1.5]">{x.ask}</p>
      </div>
      {stage >= 1 && <p className={`shrink-0 text-[13px] leading-[1.5] text-fg/90 ${appear}`}>{x.reply}</p>}
      {stage >= 2 && (
        <div className={`shrink-0 border-y border-white/[0.06] py-1.5 ${appear}`}>
          {x.steps.slice(0, running + 1).map(([t, d], j) => (
            <div key={t} className={`flex h-7 items-center gap-2 text-[12px] ${appear}`}>
              {j < running ? <Check className="size-3.5 text-ok" /> : <Loader className="size-3.5 animate-[spin_1.6s_linear_infinite] text-accent" />}
              <span className={`flex-1 ${j < running ? "" : "text-muted"}`}>{t}</span>
              {j < running && <span className="tabular-nums text-muted">{d}</span>}
            </div>
          ))}
        </div>
      )}
      {stage >= doneStage(x) && (
        <div className={`flex shrink-0 flex-col gap-2 rounded-[10px] bg-raised p-3 ${appear}`}>
          <div className="flex h-[26px] items-center gap-2">
            <Check className="size-3.5 text-ok" />
            <span className="flex-1 text-[13px] font-semibold">{x.result}</span>
            {latest && x.undo !== false && (
              <span className="flex h-[26px] items-center gap-[5px] rounded-lg bg-white/[0.09] px-2.5 text-[12px] font-medium ring-1 ring-inset ring-white/[0.07]">
                <Undo2 className="size-3" />
                Undo
              </span>
            )}
          </div>
          {x.changes.map(([a, t]) => (
            <div key={a} className="flex pl-[22px] text-[12px]">
              <span className="flex-1">{a}</span>
              <span className="tabular-nums text-muted">{t}</span>
            </div>
          ))}
        </div>
      )}
    </>
  );
}

// The app's AI panel (DESIGN.md, AI panel). Without children it shows one finished run.
export function AgentPanel({
  className = "",
  compact = false,
  draft = "",
  children,
}: {
  className?: string;
  compact?: boolean;
  /** Text being typed into the composer. */
  draft?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className={`flex flex-col overflow-hidden rounded-[20px] bg-panel text-left ring-1 ring-inset ring-white/[0.07] ${className}`}>
      <div className="flex h-[52px] shrink-0 items-center justify-between border-b border-white/[0.07] px-4">
        <span className="flex items-center gap-1.5 text-[13px] font-semibold">
          Claude Code <ChevronDown className="size-3.5 text-muted" />
        </span>
        <span className="flex gap-3.5 text-muted">
          <SquarePen className="size-3.5" />
          <X className="size-3.5" />
        </span>
      </div>
      {/* New messages land at the bottom and push the older ones up and out under a soft fade. */}
      <div
        className={`flex min-h-0 flex-1 flex-col gap-4 overflow-hidden ${children ? "justify-end [mask-image:linear-gradient(to_bottom,transparent,black_48px)]" : ""} ${compact ? "p-4" : "p-5"}`}
      >
        {children ?? <Run x={exchanges[0]} stage={doneStage(exchanges[0])} />}
      </div>
      <div className="p-4 pt-0">
        <div className="flex min-h-12 items-center justify-between gap-3 rounded-xl bg-white/[0.055] py-2.5 pl-3.5 pr-2.5 ring-1 ring-inset ring-white/[0.08]">
          {draft ? (
            <span className="min-w-0 flex-1 text-[13px] leading-[1.45]">
              {draft}
              <span className="ml-px inline-block h-[15px] w-px translate-y-[3px] bg-fg" />
            </span>
          ) : (
            <span className="text-[13px] text-muted">Ask for another edit…</span>
          )}
          <span className={`flex size-7 shrink-0 items-center justify-center rounded-full transition-colors ${draft ? "bg-fg text-bg" : "bg-white/[0.08] text-muted"}`}>
            <ArrowUp className="size-3.5" />
          </span>
        </div>
      </div>
    </div>
  );
}
