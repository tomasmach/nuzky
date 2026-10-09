import Link from "next/link";
import { Plus } from "lucide-react";
import { SectionHeading } from "./SectionHeading";
import { faq, type Faq as Item } from "@/lib/content";

export function FaqList({ items, className = "" }: { items: Item[]; className?: string }) {
  return (
    <div className={`mx-auto max-w-[820px] border-t border-white/[0.07] ${className}`}>
      {items.map(({ q, a, link }) => (
        <details key={q} className="group border-b border-white/[0.07]">
          <summary className="flex cursor-pointer list-none items-center justify-between gap-6 rounded-lg py-6 text-[17px] font-semibold tracking-[-0.01em] sm:text-[19px] [&::-webkit-details-marker]:hidden">
            {q}
            <Plus className="size-[18px] shrink-0 text-muted transition-transform duration-200 group-open:rotate-45" />
          </summary>
          <p className="-mt-1.5 max-w-[680px] pb-7 text-[15px] leading-[1.55] text-muted">
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
    <section id="faq" className="mx-auto w-full max-w-[1240px] scroll-mt-10 px-5 pt-32 sm:pt-[180px]">
      <SectionHeading center first="Before you switch." second="The usual questions." />
      <FaqList items={faq} className="mt-12" />
    </section>
  );
}
