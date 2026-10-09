import Image from "next/image";
import { GitHubLogo } from "./Logos";
import { repo } from "@/lib/site";

const links = [
  ["Editor", "#editor"],
  ["AI agents", "#agents"],
  ["Features", "#features"],
  ["Download", "#download"],
] as const;

export function Nav() {
  return (
    <header className="relative z-20 mx-auto flex h-[72px] w-full max-w-[1440px] items-center justify-between px-5 sm:px-10">
      <a href="#top" className="flex w-60 items-center gap-2.5 rounded-lg" aria-label="Nuzky home">
        <Image src="/media/icon.png" alt="" width={34} height={34} priority />
        <span className="text-[16px] font-semibold tracking-[-0.01em]">Nuzky</span>
      </a>
      <nav aria-label="Sections" className="hidden items-center gap-8 md:flex">
        {links.map(([label, href]) => (
          <a key={href} href={href} className="text-[14px] text-muted transition-colors hover:text-fg">
            {label}
          </a>
        ))}
      </nav>
      <div className="flex w-60 items-center justify-end gap-5">
        <a href={repo} className="hidden items-center gap-1.5 text-[14px] text-muted transition-colors hover:text-fg sm:flex">
          <GitHubLogo className="size-4" />
          GitHub
        </a>
        <a href="#download" className="btn-light flex h-[34px] items-center rounded-full px-4 text-[13px] font-semibold">
          Download
        </a>
      </div>
    </header>
  );
}
