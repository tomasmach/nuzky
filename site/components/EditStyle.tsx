import Image from "next/image";
import { ChevronRight, FileText, Sparkles } from "lucide-react";
import { SectionHeading } from "./SectionHeading";

// How the style learner works, left to right: the creator's own cuts, the EDIT.md it writes from them,
// and the next recording cut by an agent that follows it. Each rule is one `style learn` can write
// (crates/analysis/src/style/learn.rs): it measures caption length and position and zoom scale and rate,
// not caption looks or what is on screen.

// A recording as a bar: [kept, width in %] stretches, kept ones lit.
type Recording = { name: string; from: string; to: string; parts: [0 | 1, number][] };

const cuts: Recording[] = [
  { name: "IMG_3104.MOV", from: "4:12", to: "0:41", parts: [[1, 5], [0, 9], [1, 3], [0, 14], [1, 4], [0, 8], [1, 2], [0, 22], [1, 4], [0, 29]] },
  { name: "IMG_3117.MOV", from: "3:05", to: "0:34", parts: [[0, 6], [1, 6], [0, 12], [1, 3], [0, 18], [1, 5], [0, 7], [1, 4], [0, 39]] },
  { name: "IMG_3131.MOV", from: "5:40", to: "0:52", parts: [[1, 4], [0, 16], [1, 5], [0, 6], [1, 2], [0, 25], [1, 3], [0, 10], [1, 2], [0, 27]] },
];
const next: Recording = { name: "IMG_3188.MOV", from: "3:48", to: "0:39", parts: [[1, 4], [0, 11], [1, 4], [0, 9], [1, 3], [0, 20], [1, 4], [0, 8], [1, 2], [0, 35]] };

const rules: [string, string[]][] = [
  ["What gets cut", ["Retakes: keep the last attempt", "“um” and “like”: cut 9 of 10", "Pauses over 0.4 s"]],
  ["Captions", ["3 words a line, low in the frame"]],
  ["Zoom", ["In to 1.2×, about 4 times a minute"]],
];

function Bar({ r }: { r: Recording }) {
  return (
    <div className="flex w-full flex-col gap-1.5">
      <div className="flex justify-between text-[11.5px] tabular-nums">
        <span className="text-fg/85">{r.name}</span>
        <span className="text-muted">
          {r.from} → {r.to}
        </span>
      </div>
      <div className="flex h-6 gap-px overflow-hidden rounded-[6px]" aria-hidden>
        {r.parts.map(([kept, w], i) => (
          <span key={i} className={kept ? "bg-accent" : "bg-white/[0.08]"} style={{ width: `${w}%` }} />
        ))}
      </div>
    </div>
  );
}

function Step({ n, text, children }: { n: number; text: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col items-center justify-center md:h-[300px]">{children}</div>
      <div className="flex items-start gap-3">
        <span className="flex size-6 shrink-0 items-center justify-center rounded-full text-[12px] font-semibold ring-1 ring-inset ring-white/20">{n}</span>
        <p className="text-[15px] leading-[1.45] text-fg/85">{text}</p>
      </div>
    </div>
  );
}

function Arrow() {
  return (
    <span className="hidden size-8 items-center justify-center self-start rounded-full bg-white/[0.06] text-muted md:mt-[134px] md:flex" aria-hidden>
      <ChevronRight className="size-4" />
    </span>
  );
}

export function EditStyle() {
  return (
    <section id="style" className="mx-auto w-full max-w-[1240px] scroll-mt-10 px-5 pt-32 sm:pt-[180px]">
      <SectionHeading center first="It learns how you edit." second="Then cuts the next video your way." />
      <p className="mx-auto mt-6 max-w-[640px] text-center text-[17px] leading-[1.55] text-muted">
        Give Nuzky a few raw recordings and the videos you cut from them. It writes down how you edit, with examples from your own videos, and your AI
        follows it on every new video. Change a line, and the next cut changes with it.
      </p>

      <div className="surface mt-12 grid gap-10 rounded-3xl p-7 sm:p-9 md:grid-cols-[1fr_auto_1fr_auto_1fr] md:gap-6">
        <Step n={1} text="Your recordings and the videos you cut from them">
          <div className="flex w-full max-w-[300px] flex-col gap-5">
            {cuts.map((r) => (
              <Bar key={r.name} r={r} />
            ))}
          </div>
        </Step>
        <Arrow />
        <Step n={2} text="Your style, written down in a file you can read and change">
          <div className="w-full max-w-[300px] overflow-hidden rounded-[14px] bg-panel ring-1 ring-inset ring-white/[0.08]">
            <div className="flex h-8 items-center gap-[7px] border-b border-white/[0.06] px-3 text-[12px] text-muted">
              <FileText className="size-[13px]" />
              EDIT.md
            </div>
            <div className="flex flex-col gap-[7px] px-3.5 pb-3.5 pt-3 font-mono text-[11.5px]">
              <span className="font-semibold"># Editing style</span>
              {rules.map(([heading, lines]) => (
                <div key={heading} className="flex flex-col gap-[5px]">
                  <span className="pt-1 text-fg/60">## {heading}</span>
                  {lines.map((l) => (
                    <span key={l} className="text-fg/85">
                      <span className="text-accent">- </span>
                      {l}
                    </span>
                  ))}
                </div>
              ))}
            </div>
          </div>
        </Step>
        <Arrow />
        <Step n={3} text="Your AI cuts the next recording the way you would">
          <div className="flex w-full max-w-[300px] flex-col items-center gap-4">
            <div className="relative h-[160px] w-[90px] overflow-hidden ring-1 ring-white/15">
              <Image src="/media/ugc-small.jpg" alt="" fill sizes="90px" className="object-cover" />
              <span className="absolute inset-x-0 bottom-5 flex flex-col items-center text-[11px] font-black leading-[1.15]" style={{ filter: "drop-shadow(0 1px 4px rgb(0 0 0 / 0.7))" }}>
                ACTUALLY
                <span className="text-caption-yellow">WORKS</span>
              </span>
            </div>
            <span className="capsule flex h-[30px] items-center gap-1.5 rounded-full px-3 text-[12px] font-medium">
              <Sparkles className="size-3.5 text-accent" />
              Following EDIT.md
            </span>
            <Bar r={next} />
          </div>
        </Step>
      </div>
    </section>
  );
}
