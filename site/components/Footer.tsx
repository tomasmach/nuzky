import Image from "next/image";
import { buildFromSource, license, releaseNotes, repo } from "@/lib/site";

export function Footer() {
  const links = [
    ["GitHub", repo],
    ["Releases", releaseNotes],
    ["Building from source", buildFromSource],
    ["License", license],
  ] as const;
  return (
    <footer className="mx-auto w-full max-w-[1240px] px-5">
      <div className="flex flex-col gap-5 border-t border-white/[0.07] pb-12 pt-8 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex items-center gap-2.5 text-[13px] text-muted">
          <Image src="/media/icon.png" alt="" width={24} height={24} />
          Nuzky · Open source under GPL-3.0
        </div>
        <nav aria-label="Project" className="flex flex-wrap gap-x-7 gap-y-2">
          {links.map(([label, href]) => (
            <a key={label} href={href} className="text-[13px] text-muted transition-colors hover:text-fg">
              {label}
            </a>
          ))}
        </nav>
      </div>
    </footer>
  );
}
