import Link from "next/link";
import { Plus } from "lucide-react";
import { SectionHeading } from "./SectionHeading";
import { Waves } from "./Waves";
import { faq, type Faq as Item } from "@/lib/content";

// The questions sit on one glass panel with the silver light moving behind it (see Waves): the marker gives
// the light a point a little below the top of the panel, which stays put when a question opens.
export function FaqList({ items, className = "" }: { items: Item[]; className?: string }) {
  return (
    <div
      className={`glass mx-auto max-w-[820px] rounded-[28px] bg-[rgb(22_22_25/0.5)] p-2 backdrop-blur-[28px] backdrop-saturate-[1.6] contrast-more:bg-[#232327] contrast-more:backdrop-blur-none contrast-more:backdrop-saturate-100 ${className}`}
    >
      <span data-waves="faq" aria-hidden className="pointer-events-none absolute inset-x-0 top-[180px]" />
      {items.map(({ q, a, link }) => (
        <details
          key={q}
          className="group relative rounded-[20px] transition-colors before:absolute before:inset-x-5 before:top-0 before:h-px before:bg-white/[0.08] first-of-type:before:hidden hover:bg-white/[0.035] hover:before:hidden open:bg-white/[0.05] open:before:hidden [&:hover+details]:before:hidden [&[open]+details]:before:hidden"
        >
          <summary className="flex cursor-pointer list-none items-center justify-between gap-5 rounded-[20px] px-5 py-[18px] text-[16px] font-semibold tracking-[-0.01em] sm:px-6 sm:text-[18px] [&::-webkit-details-marker]:hidden">
            {q}
            <span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-white/[0.08] ring-1 ring-inset ring-white/[0.1]">
              <Plus className="size-4 transition-transform duration-200 group-open:rotate-45" />
            </span>
          </summary>
          <p className="-mt-1 max-w-[680px] px-5 pb-6 text-[15px] leading-[1.55] text-fg/70 sm:px-6">
            {a}
            {link && (
              <>
                {" "}
                <Link href={link.href} className="text-fg underline decoration-white/30 underline-offset-4 transition-colors hover:decoration-fg">
                  {link.label}
                </Link>
              </>
            )}
          </p>
        </details>
      ))}
    </div>
  );
}

export function Faq() {
  return (
    // The light runs edge to edge and fades out below the panel, before the download section brings its own.
    <div className="relative overflow-hidden pb-16">
      <Waves swellAt="[data-waves=faq]" intensity={1.9} className="[mask-image:linear-gradient(to_bottom,transparent,#000_240px)]" />
      <section id="faq" className="relative z-10 mx-auto w-full max-w-[1240px] scroll-mt-10 px-5 pt-32 sm:pt-[180px]">
        <SectionHeading center first="Before you switch." second="The usual questions." />
        <FaqList items={faq} className="mt-12" />
      </section>
    </div>
  );
}
