import { ArrowUp, Check, ChevronDown, SquarePen, Undo2, X } from "lucide-react";

const steps = [
  ["Transcribe 0:31 of audio", "12 s"],
  ["Remove 14 filler words", "0.4 s"],
  ["Cut 6 pauses over 0.8 s", "0.3 s"],
  ["Add captions · Bold", "0.2 s"],
];

const changes = [
  ["Cut “um” at", "00:09.4"],
  ["Cut pause at", "00:12.1"],
  ["Added 9 captions", "00:00–00:31"],
];

// The app's AI panel after a finished run (DESIGN.md, AI panel).
export function AgentPanel({ className = "", compact = false }: { className?: string; compact?: boolean }) {
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
      <div className={`flex flex-1 flex-col gap-4 ${compact ? "p-4" : "p-5"}`}>
        <div className="flex justify-end">
          <p className="max-w-[290px] rounded-[14px] rounded-br-[4px] bg-raised px-3.5 py-2.5 text-[13px] leading-[1.5]">
            Cut the ums and long pauses, then add captions in the Bold style.
          </p>
        </div>
        <p className="text-[13px] leading-[1.5] text-fg/90">On it. I&apos;ll transcribe first, then cut.</p>
        <div className="border-y border-white/[0.06] py-1.5">
          {steps.map(([t, d]) => (
            <div key={t} className="flex h-7 items-center gap-2 text-[12px]">
              <Check className="size-3.5 text-ok" />
              <span className="flex-1">{t}</span>
              <span className="tabular-nums text-muted">{d}</span>
            </div>
          ))}
        </div>
        <div className="flex flex-col gap-2 rounded-[10px] bg-raised p-3">
          <div className="flex items-center gap-2">
            <Check className="size-3.5 text-ok" />
            <span className="flex-1 text-[13px] font-semibold">21 changes</span>
            <span className="flex h-[26px] items-center gap-[5px] rounded-lg bg-white/[0.09] px-2.5 text-[12px] font-medium ring-1 ring-inset ring-white/[0.07]">
              <Undo2 className="size-3" />
              Undo
            </span>
          </div>
          {changes.map(([a, t]) => (
            <div key={a} className="flex pl-[22px] text-[12px]">
              <span className="flex-1">{a}</span>
              <span className="tabular-nums text-muted">{t}</span>
            </div>
          ))}
        </div>
      </div>
      <div className="p-4 pt-0">
        <div className="flex h-12 items-center justify-between rounded-xl bg-white/[0.055] pl-3.5 pr-2.5 ring-1 ring-inset ring-white/[0.08]">
          <span className="text-[13px] text-muted">Ask for another edit…</span>
          <span className="flex size-7 items-center justify-center rounded-full bg-white/[0.08]">
            <ArrowUp className="size-3.5 text-muted" />
          </span>
        </div>
      </div>
    </div>
  );
}
