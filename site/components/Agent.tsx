"use client";

import Image from "next/image";
import { useEffect, useRef, useState } from "react";
import { CircleCheck, Loader, Terminal } from "lucide-react";
import { AgentPanel, Run, doneStage, exchanges } from "./AgentPanel";
import { ClaudeCodeLogo, OpenAILogo } from "./Logos";
import { SectionHeading } from "./SectionHeading";

const strip = [1, 2, 3, 4, 5, 6, 7];
const TYPING = -1;

// Plays the exchanges forever while the section is on screen: the user types, the agent replies,
// works through its steps and reports, then the next request after a short look at the result. It
// starts on a finished first run, which is also what the server renders and what reduced motion keeps,
// and moves on from it almost at once so the panel is never idle for long.
function useAgentLoop(active: boolean) {
  const [n, setN] = useState(0);
  const [stage, setStage] = useState(doneStage(exchanges[0]));
  const [typed, setTyped] = useState(0);

  useEffect(() => {
    if (!active) return;
    const x = exchanges[n % exchanges.length];
    let id: number;
    if (stage === TYPING) {
      id =
        typed < x.ask.length
          ? window.setTimeout(() => setTyped((t) => t + 1), 24 + ((typed * 37) % 40))
          : window.setTimeout(() => {
              setTyped(0);
              setStage(0);
            }, 450);
    } else if (stage < doneStage(x)) {
      id = window.setTimeout(() => setStage((s) => s + 1), stage === 0 ? 700 : 850);
    } else {
      id = window.setTimeout(
        () => {
          setN((k) => k + 1);
          setStage(TYPING);
        },
        n === 0 ? 600 : 1600,
      );
    }
    return () => window.clearTimeout(id);
  }, [active, n, stage, typed]);

  return { n, stage, typed };
}

export function Agent() {
  const section = useRef<HTMLElement>(null);
  const [active, setActive] = useState(false);

  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const io = new IntersectionObserver(([e]) => setActive(e.isIntersecting), { threshold: 0.3 });
    if (section.current) io.observe(section.current);
    return () => io.disconnect();
  }, []);

  const { n, stage, typed } = useAgentLoop(active);
  const at = (k: number) => exchanges[k % exchanges.length];
  const current = at(n);
  const working = stage >= 0 && stage < doneStage(current);
  // Older runs scroll out of the panel, so two finished ones are enough history.
  const history = [n - 2, n - 1].filter((k) => k >= 0);
  const lit = working ? ((n * 3 + stage) % strip.length) + 1 : 4;

  return (
    <section id="agents" ref={section} className="mx-auto w-full max-w-[1240px] scroll-mt-10 px-5 pt-32 sm:pt-[180px]">
      <SectionHeading center first="Or let an agent cut it." second="You watch, and one undo takes it all back." />
      <div className="mt-10 flex flex-col gap-4 lg:h-[560px] lg:flex-row">
        <AgentPanel className="h-[560px] lg:h-auto lg:w-[440px] lg:shrink-0" draft={stage === TYPING ? current.ask.slice(0, typed) : ""}>
          {history.map((k) => (
            <Run key={k} x={at(k)} stage={doneStage(at(k))} latest={k === n - 1 && stage === TYPING} />
          ))}
          {stage !== TYPING && <Run key={n} x={current} stage={stage} />}
        </AgentPanel>
        <div className="flex min-w-0 flex-1 flex-col gap-4">
          <div className="surface flex flex-1 flex-col justify-center gap-5 rounded-[20px] p-7">
            <div className="capsule flex h-9 w-fit items-center gap-2 rounded-full pl-3.5 pr-1.5 text-[13px] font-medium">
              {working ? (
                <>
                  <Loader className="size-3.5 animate-[spin_1.6s_linear_infinite] text-accent" />
                  Claude Code is editing
                  <span className="flex h-[26px] items-center rounded-full bg-white/[0.09] px-2.5 text-[12px]">Stop</span>
                </>
              ) : (
                <>
                  <CircleCheck className="size-3.5 text-ok" />
                  <span className="pr-2">Your turn</span>
                </>
              )}
            </div>
            <div className="flex h-14 gap-0.5">
              {strip.map((i) => (
                <div
                  key={i}
                  className={`relative flex-1 overflow-hidden rounded-[7px] transition-shadow duration-200 ${i === lit ? "ring-2 ring-accent" : "ring-1 ring-inset ring-white/10"}`}
                >
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
                { name: "Claude Code", logo: <ClaudeCodeLogo className="size-[18px] text-claude" />, ready: true },
                { name: "Codex", logo: <OpenAILogo className="size-[18px]" />, ready: true },
                { name: "Any MCP client", logo: <Terminal className="size-[18px] text-muted" />, ready: false },
              ].map(({ name, logo, ready }) => (
                <div key={name} className="flex h-12 items-center gap-2.5 rounded-[10px] bg-raised px-3.5 text-[13px] font-medium">
                  {logo}
                  <span className="flex-1">{name}</span>
                  {ready && <CircleCheck className="size-4 text-ok" />}
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
