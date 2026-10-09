"use client";

import Image from "next/image";
import { useEffect, useRef, useState } from "react";
import { Check, Sparkles, Undo2 } from "lucide-react";
import { Fit } from "./Fit";
import { GitHubLogo } from "./Logos";
import { NotifyForm } from "./NotifyForm";
import { notifyNote } from "@/lib/content";
import { fitSizes } from "@/lib/fit";
import { usePlaying } from "@/lib/playing";
import { repo } from "@/lib/site";

const shadow = "drop-shadow(0 2px 8px rgb(0 0 0 / 0.7))";

function Reel({ src, w, opacity, children }: { src: string; w: number; opacity: number; children?: React.ReactNode }) {
  const h = Math.round((w * 16) / 9);
  return (
    <div className="relative shrink-0 overflow-hidden rounded-[14px] ring-1 ring-white/10" style={{ width: w, height: h, opacity }}>
      <Image src={src} alt="" fill loading="eager" sizes={fitSizes(w, 1060)} className="object-cover" />
      {children}
    </div>
  );
}

type CaptionStyle = "Bold" | "Clean" | "Pop";

const styles: [CaptionStyle, string][] = [
  ["Bold", "text-caption-yellow"],
  ["Clean", "text-fg"],
  ["Pop", "text-accent"],
];

const lines = [
  ["Okay", "this", "serum"],
  ["actually", "works"],
];
const WORDS = 5;
// The caption opens on "serum", the word the design and the server render show lit.
const FIRST_SPOKEN = 2;

// The spoken word walks through the caption the way it does in the app's preview, with a breath at the end.
// It only walks while the hero plays.
function useSpokenWord(on: boolean) {
  const [spoken, setSpoken] = useState(FIRST_SPOKEN);
  useEffect(() => {
    if (!on) return;
    let id = 0;
    const next = (i: number) => {
      setSpoken(i);
      id = window.setTimeout(() => next((i + 1) % WORDS), i === WORDS - 1 ? 1300 : 380);
    };
    id = window.setTimeout(() => next(FIRST_SPOKEN + 1), 380);
    return () => window.clearTimeout(id);
  }, [on]);
  return spoken;
}

function Caption({ style, highlight, playing }: { style: CaptionStyle; highlight: boolean; playing: boolean }) {
  const spoken = useSpokenWord(highlight && playing);
  const loud = style !== "Clean";
  const word = (w: string, i: number) => {
    const lit = highlight && i === spoken;
    const look =
      style === "Bold"
        ? lit && "text-caption-yellow"
        : style === "Pop"
          ? lit
            ? "-rotate-2 rounded-[5px] bg-accent shadow-[0_0_0_4px_var(--color-accent)]"
            : "[text-shadow:3px_3px_0_var(--color-accent)]"
          : highlight && !lit && "text-white/55";
    return (
      <span key={w} className={`inline-block transition-[color,background-color,box-shadow,text-shadow,transform] duration-150 ${look || ""}`}>
        {loud ? w.toUpperCase() : w}
      </span>
    );
  };
  let i = 0;
  return (
    <div className="absolute inset-x-0 top-[318px] flex justify-center">
      <div
        className={`flex flex-col items-center gap-y-0.5 ${
          loud ? "text-[25px] font-black leading-[1.12] tracking-[-0.015em]" : "rounded-[10px] bg-black/60 px-3 py-1.5 text-[22px] font-semibold leading-[1.2] tracking-[-0.01em]"
        }`}
        style={loud ? { filter: shadow } : undefined}
      >
        {lines.map((line) => (
          <span key={line[0]} className="flex gap-x-[0.26em]">
            {line.map((w) => word(w, i++))}
          </span>
        ))}
      </div>
    </div>
  );
}

export function Hero() {
  const [style, setStyle] = useState<CaptionStyle>("Bold");
  const [highlight, setHighlight] = useState(true);
  const section = useRef<HTMLElement>(null);
  const playing = usePlaying(section);

  return (
    <section ref={section} className="relative">
      <div className="relative z-10 flex flex-col items-center px-5 pt-16 text-center sm:pt-24">
        <h1 className="heading-xl">
          You just wanted to{"\u00a0"}edit.
          <br />
          <span className="text-subtle">Now you can.</span>
        </h1>
        <p className="mt-7 max-w-[600px] text-[17px] text-muted sm:text-[20px]">
          A free video editor. AI writes the captions in your language and cuts out the ums, the pauses and the retakes.
        </p>

        <Fit width={1060} height={498} waves="swell" className="mt-14 sm:mt-16">
          <div className="flex h-full items-center justify-center gap-5">
            <Reel src="/media/reel-hike.jpg" w={150} opacity={0.35} />
            <Reel src="/media/reel-skate.jpg" w={200} opacity={0.6}>
              <div className="absolute inset-x-0 bottom-[56px] flex flex-col items-center text-[22px] font-black leading-[1.1] tracking-[-0.02em]" style={{ filter: shadow }}>
                <span>KICKFLIP</span>
                <span className="text-caption-yellow">FIRST TRY</span>
              </div>
            </Reel>

            {/* The UGC clip in the middle, selected the way the preview shows a selection. */}
            <div className="relative shrink-0" style={{ width: 280, height: 498 }}>
              <div className="absolute inset-0 overflow-hidden ring-1 ring-white/20">
                <Image
                  src="/media/ugc.jpg"
                  alt="A creator holding a serum bottle, talking to the camera"
                  fill
                  loading="eager"
                  fetchPriority="high"
                  sizes={fitSizes(280, 1060)}
                  className="object-cover"
                />
                <div className="absolute left-3.5 top-4 flex items-center gap-2" style={{ filter: shadow }}>
                  <Image src="/media/avatar.jpg" alt="" width={26} height={26} loading="eager" className="size-[26px] rounded-full object-cover ring-[1.5px] ring-white" />
                  <span className="text-[13px] font-semibold">@mia.skin</span>
                </div>
                <Caption style={style} highlight={highlight} playing={playing} />
              </div>
              <div className="pointer-events-none absolute inset-0 outline outline-[1.5px] outline-fg" />
              {[
                "-left-[5px] -top-[5px]",
                "-right-[5px] -top-[5px]",
                "-bottom-[5px] -left-[5px]",
                "-bottom-[5px] -right-[5px]",
              ].map((pos) => (
                <span key={pos} className={`absolute size-2.5 border border-bg bg-fg ${pos}`} />
              ))}
              <span className="absolute -top-[18px] left-1/2 h-[18px] w-px -translate-x-1/2 bg-fg" />
              <span className="absolute -top-[30px] left-1/2 size-3 -translate-x-1/2 rounded-full border border-bg bg-fg" />
            </div>

            <Reel src="/media/reel-coffee.jpg" w={200} opacity={0.6}>
              <div className="absolute inset-x-0 bottom-[56px] flex justify-center">
                <div className="flex flex-col items-center rounded-md bg-black/70 px-2 py-1 text-[20px] font-semibold italic leading-[1.15]">
                  <span>slow</span>
                  <span>mornings</span>
                </div>
              </div>
            </Reel>
            <Reel src="/media/reel-concert.jpg" w={150} opacity={0.35} />
          </div>

          {/* Floating glass: the agent's run on the left, the caption style on the right. */}
          <div className="glass absolute left-9 top-[264px] hidden w-[286px] flex-col gap-2.5 rounded-[22px] px-4 pb-4 pt-3.5 text-left md:flex">
            <div className="flex items-center gap-[7px]">
              <Sparkles className="size-3.5 text-accent" />
              <span className="flex-1 text-[13px] font-semibold">Claude Code</span>
              <span className="flex h-6 items-center gap-1 rounded-full bg-white/[0.12] px-2.5 text-[11px] font-medium">
                <Undo2 className="size-[11px]" />
                Undo
              </span>
            </div>
            {["Cut 14 filler words", "Removed 6 long pauses", `Added captions · ${style}`].map((t) => (
              <div key={t} className="flex items-center gap-2 text-[12px]">
                <Check className="size-[13px] text-ok" />
                {t}
              </div>
            ))}
          </div>
          <div className="glass absolute left-[754px] top-[156px] hidden w-[262px] flex-col gap-3 rounded-[22px] p-4 text-left md:flex">
            <span className="text-[13px] font-semibold">Caption style</span>
            <div className="flex gap-2" role="group" aria-label="Caption style">
              {styles.map(([n, c]) => (
                <button
                  key={n}
                  type="button"
                  aria-pressed={style === n}
                  onClick={() => setStyle(n)}
                  className={`flex h-11 flex-1 cursor-pointer items-center justify-center rounded-xl bg-black/35 text-[14px] font-extrabold transition-shadow ${c} ${
                    style === n ? "ring-2 ring-accent" : "ring-1 ring-white/10 hover:ring-white/30"
                  }`}
                >
                  {n}
                </button>
              ))}
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={highlight}
              onClick={() => setHighlight((h) => !h)}
              className="flex cursor-pointer items-center justify-between rounded-md text-left"
            >
              <span className="text-[12px] text-fg/85">Highlight the spoken word</span>
              <span className={`flex h-5 w-[34px] items-center rounded-full p-0.5 transition-colors ${highlight ? "bg-accent-strong" : "bg-white/20"}`}>
                <span className={`size-4 rounded-full bg-white shadow-[0_1px_2px_rgb(0_0_0/0.3)] transition-transform duration-200 ${highlight ? "translate-x-[14px]" : ""}`} />
              </span>
            </button>
          </div>
        </Fit>

        <div className="mt-14 flex flex-col items-center gap-3.5 sm:mt-16">
          {/* Not released yet: the email field takes the download's place until the first build ships. */}
          <div className="flex w-full max-w-[400px] flex-col items-center gap-3 sm:max-w-none sm:flex-row sm:items-start sm:justify-center">
            <NotifyForm id="notify" noteId="hero-notify-note" />
            <a href={repo} className="glass flex h-12 shrink-0 items-center gap-2 rounded-full px-[22px] text-[15px] font-semibold transition-colors hover:bg-white/10">
              <GitHubLogo className="size-[17px]" />
              Star on GitHub
            </a>
          </div>
          <p id="hero-notify-note" className="max-w-[460px] text-[14px] leading-[1.5] text-muted">
            Coming soon for Mac, Windows and Linux. {notifyNote}
          </p>
        </div>
      </div>
    </section>
  );
}
