import Image from "next/image";
import { CircleCheck } from "lucide-react";
import { SectionHeading } from "./SectionHeading";

function Step({ n, title, text, children }: { n: number; title: string; text: string; children: React.ReactNode }) {
  return (
    <div className="surface flex h-[440px] flex-col overflow-hidden rounded-[20px]">
      <div className="flex flex-1 flex-col items-center justify-center p-7">{children}</div>
      <div className="flex flex-col gap-1.5 px-7 pb-7">
        <span className="flex size-6 items-center justify-center rounded-full text-[12px] font-semibold ring-1 ring-inset ring-white/20">{n}</span>
        <h3 className="text-[18px] font-semibold">{title}</h3>
        <p className="text-[14px] leading-[1.45] text-muted">{text}</p>
      </div>
    </div>
  );
}

export function Steps() {
  return (
    <section className="mx-auto w-full max-w-[1240px] px-5 pt-32 sm:pt-[180px]">
      <SectionHeading center first="From raw clips" second="to a finished cut." />
      <div className="mt-10 grid gap-4 md:grid-cols-3">
        <Step n={1} title="Drop in your clips" text="Nuzky opens video from any phone or camera, including HEVC and variable frame rate.">
          <div className="grid grid-cols-3 gap-2">
            {[1, 2, 3, 4, 5, 6].map((i) => (
              <div key={i} className="relative size-24 overflow-hidden rounded-[10px]">
                <Image src={`/media/tile-${i}.jpg`} alt="" fill sizes="96px" className="object-cover" />
              </div>
            ))}
          </div>
        </Step>
        <Step n={2} title="Caption it" text="Whisper writes the captions on your computer. Pick a style and fix any word it misheard.">
          <div className="flex w-full flex-col gap-2.5">
            {[
              ["00:04", "and caught first light"],
              ["00:07", "on the ridge at six"],
              ["00:11", "we made it to the top"],
            ].map(([t, line], i) => (
              <div key={t} className={`flex gap-3 rounded-[10px] px-3 py-2.5 ${i === 1 ? "bg-accent/[0.18] ring-1 ring-inset ring-accent/50" : "bg-white/[0.03]"}`}>
                <span className="text-[12px] tabular-nums text-muted">{t}</span>
                <span className="text-[14px]">{line}</span>
              </div>
            ))}
          </div>
        </Step>
        <Step n={3} title="Export anywhere" text="Choose Reels and TikTok or any size you need. The file matches the preview.">
          <div className="flex flex-col items-center gap-3.5">
            <div className="relative h-[196px] w-[110px] ring-1 ring-white/15">
              <Image src="/media/export-file.jpg" alt="" fill sizes="110px" className="object-cover" />
            </div>
            <span className="flex h-[30px] items-center gap-1.5 rounded-full bg-white/[0.075] px-3 text-[12px] font-medium">
              <CircleCheck className="size-3.5 text-ok" />
              morning-run.mp4 · 12.4 MB
            </span>
          </div>
        </Step>
      </div>
    </section>
  );
}
