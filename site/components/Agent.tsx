import Image from "next/image";
import { CircleCheck, Loader, Terminal } from "lucide-react";
import { AgentPanel } from "./AgentPanel";
import { SectionHeading } from "./SectionHeading";

const strip = [1, 2, 3, 4, 5, 6, 7];

export function Agent() {
  return (
    <section id="agents" className="mx-auto w-full max-w-[1240px] scroll-mt-10 px-5 pt-32 sm:pt-[180px]">
      <SectionHeading center first="Or let an agent cut it." second="You watch, and one undo takes it all back." />
      <div className="mt-10 flex flex-col gap-4 lg:h-[560px] lg:flex-row">
        <AgentPanel className="lg:w-[440px] lg:shrink-0" />
        <div className="flex min-w-0 flex-1 flex-col gap-4">
          <div className="surface flex flex-1 flex-col justify-center gap-5 rounded-[20px] p-7">
            <div className="capsule flex h-9 w-fit items-center gap-2 rounded-full pl-3.5 pr-1.5 text-[13px] font-medium">
              <Loader className="size-3.5 animate-[spin_1.6s_linear_infinite] text-accent" />
              Claude Code is editing
              <span className="flex h-[26px] items-center rounded-full bg-white/[0.09] px-2.5 text-[12px]">Stop</span>
            </div>
            <div className="flex h-14 gap-0.5">
              {strip.map((i) => (
                <div key={i} className={`relative flex-1 overflow-hidden rounded-[7px] ${i === 4 ? "ring-2 ring-accent" : "ring-1 ring-inset ring-white/10"}`}>
                  <Image src={`/media/strip-${i}.jpg`} alt="" fill sizes="200px" className="object-cover" />
                </div>
              ))}
            </div>
            <p className="max-w-[420px] text-[14px] leading-[1.5] text-muted">
              Every change goes through the same undo as yours. Your controls lock while it works and come back when it stops.
            </p>
          </div>
          <div className="surface flex flex-col gap-3.5 rounded-[20px] px-7 py-6">
            <span className="text-[15px] font-semibold">Connect agent</span>
            <div className="grid gap-3 sm:grid-cols-3">
              {[
                ["Claude Code", true],
                ["Codex", true],
                ["Any MCP client", false],
              ].map(([name, ready]) => (
                <div key={name as string} className="flex h-12 items-center justify-between rounded-[10px] bg-raised px-3.5 text-[13px] font-medium">
                  {name}
                  {ready ? <CircleCheck className="size-4 text-ok" /> : <Terminal className="size-4 text-muted" />}
                </div>
              ))}
            </div>
            <span className="text-[13px] text-muted">One click sets up Claude Code and Codex.</span>
          </div>
        </div>
      </div>
    </section>
  );
}
