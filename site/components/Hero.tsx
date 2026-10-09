import Image from "next/image";
import { Check, Sparkles, Undo2 } from "lucide-react";
import { Fit } from "./Fit";
import { AppleLogo, GitHubLogo } from "./Logos";
import { Waves } from "./Waves";
import { releases, repo } from "@/lib/site";

const shadow = "drop-shadow(0 2px 8px rgb(0 0 0 / 0.7))";

function Reel({ src, w, opacity, children }: { src: string; w: number; opacity: number; children?: React.ReactNode }) {
  const h = Math.round((w * 16) / 9);
  return (
    <div className="relative shrink-0 overflow-hidden rounded-[14px] ring-1 ring-white/10" style={{ width: w, height: h, opacity }}>
      <Image src={src} alt="" fill sizes={`${w * 2}px`} className="object-cover" />
      {children}
    </div>
  );
}

export function Hero() {
  return (
    <section id="top" className="relative -mt-[72px] overflow-hidden pt-[72px]">
      <Waves height={0.4} intensity={1.45} />
      <div className="relative z-10 flex flex-col items-center px-5 pt-16 text-center sm:pt-24">
        <h1 className="heading-xl">
          Make the reel.
          <br />
          <span className="text-subtle">Skip the subscription.</span>
        </h1>
        <p className="mt-7 max-w-xl text-[17px] text-muted sm:text-[20px]">A free, open source video editor with AI built in.</p>

        <Fit width={1060} height={498} className="mt-14 sm:mt-16">
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
                <Image src="/media/ugc.jpg" alt="A creator holding a serum bottle, talking to the camera" fill priority sizes="560px" className="object-cover" />
                <div className="absolute left-3.5 top-4 flex items-center gap-2" style={{ filter: shadow }}>
                  <Image src="/media/avatar.jpg" alt="" width={26} height={26} className="size-[26px] rounded-full object-cover ring-[1.5px] ring-white" />
                  <span className="text-[13px] font-semibold">@mia.skin</span>
                </div>
                <div className="absolute inset-x-0 top-[318px] flex flex-col items-center text-[25px] font-black leading-[1.12] tracking-[-0.015em]" style={{ filter: shadow }}>
                  <span>
                    OKAY THIS <span className="text-ok">SERUM</span>
                  </span>
                  <span>ACTUALLY WORKS</span>
                </div>
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
            {["Cut 14 filler words", "Removed 6 long pauses", "Added captions · Bold"].map((t) => (
              <div key={t} className="flex items-center gap-2 text-[12px]">
                <Check className="size-[13px] text-ok" />
                {t}
              </div>
            ))}
          </div>
          <div className="glass absolute left-[754px] top-[156px] hidden w-[262px] flex-col gap-3 rounded-[22px] p-4 text-left md:flex">
            <span className="text-[13px] font-semibold">Caption style</span>
            <div className="flex gap-2">
              {[
                ["Bold", "text-caption-yellow", true],
                ["Clean", "text-fg", false],
                ["Pop", "text-accent", false],
              ].map(([n, c, on]) => (
                <span
                  key={n as string}
                  className={`flex h-11 flex-1 items-center justify-center rounded-xl bg-black/35 text-[14px] font-extrabold ${c} ${on ? "ring-2 ring-accent" : "ring-1 ring-white/10"}`}
                >
                  {n}
                </span>
              ))}
            </div>
            <div className="flex items-center justify-between">
              <span className="text-[12px] text-fg/85">Highlight the spoken word</span>
              <span className="flex h-5 w-[34px] items-center justify-end rounded-full bg-accent-strong p-0.5">
                <span className="size-4 rounded-full bg-white" />
              </span>
            </div>
          </div>
        </Fit>

        <div className="mt-14 flex flex-col items-center gap-3.5 sm:mt-16">
          <div className="flex flex-wrap items-center justify-center gap-3">
            <a href={releases} className="btn-light flex h-12 items-center gap-2 rounded-full px-6 text-[15px] font-semibold">
              <AppleLogo className="size-[17px] -translate-y-px" />
              Download for macOS
            </a>
            <a href={repo} className="glass flex h-12 items-center gap-2 rounded-full px-[22px] text-[15px] font-semibold transition-colors hover:bg-white/10">
              <GitHubLogo className="size-[17px]" />
              Star on GitHub
            </a>
          </div>
          <p className="text-[14px] text-muted">Also for Windows and Linux. No account, no watermark.</p>
        </div>
      </div>
    </section>
  );
}
