import Image from "next/image";
import { ArrowDownToLine } from "lucide-react";
import { platformLogo } from "./Logos";
import { Waves } from "./Waves";
import { buildFromSource, license, platforms, releaseNotes, version } from "@/lib/site";

export function Download() {
  return (
    <section id="download" className="relative mt-32 overflow-hidden pb-28 pt-[136px] sm:mt-0">
      <Waves height={0.36} intensity={1} />
      <div className="relative z-10 flex flex-col items-center gap-11 px-5">
        <div className="flex flex-col items-center gap-5">
          <Image src="/media/icon.png" alt="Nuzky app icon" width={140} height={140} />
          <h2 className="text-center text-[44px] font-semibold leading-none tracking-[-0.04em] sm:text-[56px]">Download Nuzky.</h2>
        </div>

        <div className="flex flex-col items-center gap-[22px]">
          {/* Not released yet: the downloads keep their place but stay dim and inert until the first build ships. */}
          <div className="glass flex flex-col gap-1 rounded-[22px] p-2 sm:flex-row sm:items-center sm:gap-0">
            {platforms.map((p, i) => {
              const Logo = platformLogo[p.id];
              const primary = i === 0;
              return (
                <div key={p.id} className="flex items-center sm:contents">
                  {i > 0 && <span className="mx-0 hidden h-7 w-px bg-white/[0.08] sm:block" />}
                  <span
                    aria-disabled="true"
                    role="link"
                    className={`flex w-full select-none items-center gap-3.5 rounded-2xl py-2 pl-3.5 pr-2 opacity-40 sm:w-auto ${
                      primary ? "bg-white/5" : "sm:pl-[18px] sm:pr-5"
                    }`}
                  >
                    <Logo className="size-[22px] shrink-0" />
                    <span className="flex flex-1 flex-col gap-0.5 text-left">
                      <span className="text-[15px] font-semibold leading-tight">{p.name}</span>
                      <span className="text-[12.5px] text-muted">{p.detail}</span>
                    </span>
                    {primary ? (
                      <span className="btn-light pointer-events-none ml-4 flex h-10 items-center gap-[7px] rounded-full pl-4 pr-[18px] text-[14px] font-semibold">
                        <ArrowDownToLine className="size-[15px]" />
                        Download
                      </span>
                    ) : (
                      <span className="ml-4 flex size-8 items-center justify-center rounded-full bg-white/[0.06] ring-1 ring-inset ring-white/[0.08]">
                        <ArrowDownToLine className="size-[15px]" />
                        <span className="sr-only">Download for {p.name}</span>
                      </span>
                    )}
                  </span>
                </div>
              );
            })}
          </div>

          <p className="flex flex-wrap items-center justify-center gap-x-2.5 gap-y-1 text-[13px] text-subtle">
            <span className="text-fg">Coming soon</span>
            <span>·</span>
            <span>Version {version}</span>
            <span>·</span>
            <a href={releaseNotes} className="text-muted transition-colors hover:text-fg">
              Release notes
            </a>
            <span>·</span>
            <a href={buildFromSource} className="text-muted transition-colors hover:text-fg">
              Build from source
            </a>
            <span className="hidden sm:inline">·</span>
            <a href={license} className="transition-colors hover:text-fg">
              GPL-3.0
            </a>
          </p>
        </div>
      </div>
    </section>
  );
}
