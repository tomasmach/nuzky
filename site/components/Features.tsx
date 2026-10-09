import Image from "next/image";
import { ArrowRight, Check, Equal, FileText, HardDrive, MoveHorizontal, Scale, Scissors, Sparkles, Type } from "lucide-react";
import { Fit } from "./Fit";
import { GitHubLogo } from "./Logos";
import { SectionHeading } from "./SectionHeading";
import { Waves } from "./Waves";
import { fitSizes } from "@/lib/fit";
import { license, repo } from "@/lib/site";

function Tile({ title, text, wide = false, tall = false, backdrop, children }: { title: string; text: string; wide?: boolean; tall?: boolean; backdrop?: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className={`surface relative flex flex-col overflow-hidden rounded-3xl ${tall ? "min-h-[400px]" : "min-h-[360px]"} ${wide ? "md:col-span-2" : ""}`}>
      {backdrop}
      <div className="relative flex flex-1 flex-col items-center justify-center gap-3.5 px-6 py-8">{children}</div>
      <div className="relative flex flex-col gap-1.5 px-[30px] pb-7">
        <h3 className="text-[18px] font-semibold tracking-[-0.01em]">{title}</h3>
        <p className="text-[14px] leading-[1.45] text-muted">{text}</p>
      </div>
    </div>
  );
}

function Chip({ children }: { children: React.ReactNode }) {
  return <span className="flex h-6 items-center rounded-full bg-white/[0.06] px-2.5 text-[11px] font-medium text-fg/80 ring-1 ring-inset ring-white/[0.08]">{children}</span>;
}

function FrameAccurate() {
  const cut = 340;
  const bars = Array.from({ length: 113 }, (_, k) => 3 + Math.round(24 * Math.abs(Math.sin(k * 0.41) * Math.cos(k * 0.13 + 1))));
  return (
    <Fit width={680} height={236}>
      <div className="relative size-full">
        {Array.from({ length: 34 }, (_, i) => (
          <span key={i} className="absolute w-px" style={{ left: i * 20, top: i % 5 ? 50 : 42, height: i % 5 ? 6 : 14, background: i % 5 ? "rgb(255 255 255 / 0.15)" : "rgb(255 255 255 / 0.35)" }} />
        ))}
        {Array.from({ length: 7 }, (_, i) => (
          <span key={i} className="absolute top-10 text-[10px] tabular-nums text-subtle" style={{ left: i * 100 + 4 }}>
            {354 + i * 5}
          </span>
        ))}
        {[
          ["clip-a", 0, cut - 2, "rounded-l-[10px] rounded-r-[4px] ring-1 ring-inset ring-white/10"],
          ["clip-b", cut + 2, 680 - cut - 2, "rounded-l-[4px] rounded-r-[10px] ring-2 ring-accent"],
        ].map(([img, x, w, cls]) => (
          <div key={img as string} className={`absolute top-[76px] h-24 overflow-hidden ${cls}`} style={{ left: x as number, width: w as number }}>
            <Image src={`/media/${img}.jpg`} alt="" fill sizes={fitSizes(340, 680, 88)} className="object-cover" />
            {Array.from({ length: 6 }, (_, k) => (
              <span key={k} className="absolute inset-y-0 w-px bg-black/40" style={{ left: (k + 1) * 56 }} />
            ))}
          </div>
        ))}
        <div className="absolute inset-x-0 top-[180px] flex h-[30px] items-center gap-0.5">
          {bars.map((h, k) => (
            <span key={k} className="w-1 shrink-0 rounded-[1px]" style={{ height: h, background: k * 6 < cut ? "rgb(48 209 88 / 0.5)" : "rgb(48 209 88 / 0.8)" }} />
          ))}
        </div>
        <span className="absolute top-[30px] h-[186px] w-[1.5px] bg-white" style={{ left: cut - 0.75 }} />
        <span className="absolute top-6 size-3 rounded-full bg-white" style={{ left: cut - 6 }} />
        <div className="glass absolute -top-1.5 flex h-[34px] w-[164px] items-center justify-center gap-2 rounded-full" style={{ left: cut - 82 }}>
          <span className="text-[13px] font-semibold tabular-nums">00:12.04</span>
          <span className="text-[12px] text-muted">frame 371</span>
        </div>
      </div>
    </Fit>
  );
}

function Upright() {
  return (
    <>
      <div className="relative h-[210px] w-[300px]">
        <div className="absolute left-[37px] top-[29px] h-[150px] w-[84px] -rotate-90 overflow-hidden rounded-lg opacity-50 ring-1 ring-white/20">
          <Image src="/media/selfie.jpg" alt="" fill sizes="84px" className="object-cover" />
        </div>
        <span className="absolute left-[150px] top-[90px] flex size-7 items-center justify-center rounded-full bg-white/[0.08]">
          <ArrowRight className="size-3.5" />
        </span>
        <div className="absolute left-[188px] top-[7px] h-[196px] w-[110px] overflow-hidden rounded-[10px] ring-1 ring-white/20">
          <Image src="/media/selfie.jpg" alt="Selfie video shown the right way up" fill sizes="110px" className="object-cover" />
        </div>
      </div>
      <div className="flex gap-1.5">
        <Chip>HEVC</Chip>
        <Chip>Variable fps</Chip>
        <Chip>Front camera</Chip>
      </div>
    </>
  );
}

function SameRender() {
  const frame = (label: string) => (
    <div className="flex flex-col items-center gap-2.5">
      <div className="relative h-[150px] w-[84px] ring-1 ring-white/15">
        <Image src="/media/ugc-small.jpg" alt="" fill sizes="84px" className="object-cover" />
        <span className="absolute inset-x-0 bottom-6 mx-auto flex w-fit flex-col items-center rounded bg-black/75 px-1.5 py-[3px] text-[9px] font-black leading-[1.15]">
          ACTUALLY
          <span className="text-ok">WORKS</span>
        </span>
      </div>
      <span className="text-[12px] text-muted">{label}</span>
    </div>
  );
  return (
    <div className="flex items-center gap-3.5">
      {frame("Preview")}
      <span className="-mt-6 flex size-[30px] items-center justify-center rounded-full bg-white/[0.07]">
        <Equal className="size-3.5" />
      </span>
      {frame("Export")}
    </div>
  );
}

function EditFile() {
  const lines = ["Hook in the first 1.5 s", "Cut pauses over 0.4 s", "Captions: Bold, 3 words a line", "Zoom in on the product"];
  return (
    <div className="w-[290px] overflow-hidden rounded-[14px] bg-panel ring-1 ring-inset ring-white/[0.08]">
      <div className="flex h-8 items-center gap-[7px] border-b border-white/[0.06] px-3 text-[12px] text-muted">
        <FileText className="size-[13px]" />
        EDIT.md
      </div>
      <div className="flex flex-col gap-[7px] px-3.5 pb-3.5 pt-3 font-mono text-[11.5px]">
        <span className="font-semibold"># How Mia edits</span>
        {lines.map((l, i) => (
          <span key={l} className={i === 3 ? "text-fg/50" : "text-fg/85"}>
            <span className="text-accent">- </span>
            {l}
          </span>
        ))}
      </div>
    </div>
  );
}

function Loudness() {
  return (
    <>
      <div className="flex items-end gap-1.5">
        <span className="text-[64px] font-semibold leading-none tracking-[-0.04em]">−14</span>
        <span className="pb-2 text-[16px] font-medium text-muted">LUFS</span>
      </div>
      <div className="flex h-9 items-end gap-[3px]" aria-hidden>
        {Array.from({ length: 30 }, (_, k) => (
          <span
            key={k}
            className="w-[5px] rounded-[1.5px]"
            style={{
              height: 8 + Math.round(26 * Math.abs(Math.sin(k * 0.5 + 0.4)) * (k < 26 ? 1 : 0.4)),
              background: k < 22 ? "var(--color-ok)" : k < 26 ? "var(--color-warn)" : "rgb(255 255 255 / 0.15)",
            }}
          />
        ))}
      </div>
      <div className="flex gap-1.5">
        <Chip>Reels</Chip>
        <Chip>TikTok</Chip>
        <Chip>Shorts</Chip>
      </div>
    </>
  );
}

function History() {
  const rows = [
    [Scissors, "Split IMG_2044.MOV", "now", 1],
    [Type, "Edit caption “first light”", "12 s", 0.75],
    [MoveHorizontal, "Trim IMG_2041.MOV", "40 s", 0.5],
    [Sparkles, "Claude Code · 21 changes", "2 min", 0.3],
  ] as const;
  return (
    <div className="flex w-[290px] flex-col items-center gap-3">
      <span className="capsule flex h-9 items-center gap-2 rounded-full px-4 text-[13px]">
        <span className="font-semibold">Morning run</span>
        <span className="text-[12px] text-muted">Saved</span>
        <Check className="size-3.5 text-ok" />
      </span>
      <div className="w-full rounded-[14px] bg-panel p-1.5 ring-1 ring-inset ring-white/[0.07]">
        {rows.map(([I, t, when, o], i) => (
          <div key={t} className={`flex h-8 items-center gap-[9px] rounded-lg px-2.5 text-[12px] ${i === 0 ? "bg-white/5" : ""}`} style={{ opacity: o }}>
            <I className={`size-[13px] ${I === Sparkles ? "text-accent" : "text-muted"}`} />
            <span className="flex-1 truncate">{t}</span>
            <span className="text-[11px] text-muted">{when}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function Free() {
  return (
    <div className="flex w-full flex-col items-start gap-8 px-2 sm:flex-row sm:items-center sm:gap-12 sm:px-8">
      <div className="flex flex-col">
        <span className="text-[96px] font-semibold leading-none tracking-[-0.05em] sm:text-[120px]">$0</span>
        <span className="text-[15px] text-muted">per month, forever</span>
      </div>
      <div className="flex flex-col gap-4">
        <p className="text-[30px] font-semibold leading-[1.15] tracking-[-0.03em] sm:text-[38px]">
          <span className="text-fg/70">No account,</span>
          <br />
          <span className="text-fg/70">no watermark,</span>
          <br />
          no subscription.
        </p>
        <div className="flex flex-wrap gap-2">
          {[
            [<Scale key="s" className="size-[13px]" />, "GPL-3.0", license],
            [<HardDrive key="h" className="size-[13px]" />, "Runs locally", undefined],
            [<GitHubLogo key="g" className="size-[13px]" />, "Open on GitHub", repo],
          ].map(([icon, label, href]) => {
            const cls = "glass flex h-8 items-center gap-[7px] rounded-full px-[13px] text-[12px] font-medium";
            return href ? (
              <a key={label as string} href={href as string} className={`${cls} transition-colors hover:bg-white/10`}>
                {icon}
                {label as string}
              </a>
            ) : (
              <span key={label as string} className={cls}>
                {icon}
                {label as string}
              </span>
            );
          })}
        </div>
      </div>
    </div>
  );
}

export function Features() {
  return (
    <section id="features" className="mx-auto w-full max-w-[1240px] scroll-mt-10 px-5 pt-32 sm:pt-[180px]">
      <SectionHeading center first="Built like a pro tool." second="Without the price of one." />
      <div className="mt-12 grid gap-4 md:grid-cols-3">
        <Tile wide tall title="Frame accurate" text="Every cut lands on the exact frame, even in variable frame rate video from a phone.">
          <FrameAccurate />
        </Tile>
        <Tile tall title="Opens the right way up" text="Nuzky reads rotation and front camera mirroring from the file and applies them to the preview, thumbnails and export.">
          <Upright />
        </Tile>
        <Tile title="What you see is what you export" text="Preview and export share one renderer.">
          <SameRender />
        </Tile>
        <Tile title="Learns how you edit" text="Nuzky studies your finished cuts and writes an EDIT.md that agents follow.">
          <EditFile />
        </Tile>
        <Tile title="Ready to post" text="1080 × 1920 at 30 fps, with the sound levelled to −14 LUFS.">
          <Loudness />
        </Tile>
        <Tile tall title="Saves every edit" text="A crash or a closed window never costs you a cut.">
          <History />
        </Tile>
        <Tile wide tall title="Free, for good" text="Open source under GPL-3.0. Your videos never leave your computer." backdrop={<Waves height={0.3} intensity={0.5} />}>
          <Free />
        </Tile>
      </div>
    </section>
  );
}
