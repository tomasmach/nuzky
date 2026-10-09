import type { Metadata } from "next";
import Image from "next/image";
import { Check } from "lucide-react";
import { EditorMock } from "@/components/EditorMock";
import { FaqList } from "@/components/Faq";
import { Fit } from "@/components/Fit";
import { Footer } from "@/components/Footer";
import { JsonLd } from "@/components/JsonLd";
import { GitHubLogo } from "@/components/Logos";
import { Nav } from "@/components/Nav";
import { SectionHeading } from "@/components/SectionHeading";
import { Waves } from "@/components/Waves";
import { capcutChecked, capcutFaq, capcutSources, comparison, fromCapcut, pickCapcut, pickNuzky } from "@/lib/content";
import { repo, url } from "@/lib/site";

const path = "/capcut-alternative";
const title = "Nuzky vs CapCut: the open source CapCut alternative";
const description = "Free auto captions, no watermark and no subscription. See how Nuzky, a free and open source video editor, compares with CapCut.";

const shareImage = (src: string) => ({
  url: src,
  width: 2400,
  height: 1260,
  alt: "Nuzky: Make the reel. Skip the subscription. A free, open source video editor with AI built in.",
});

export const metadata: Metadata = {
  title: { absolute: "Free, open source CapCut alternative with no watermark | Nuzky" },
  description,
  alternates: { canonical: path, types: { "text/markdown": `${path}.md` } },
  // A page that sets its own Open Graph fields loses the root share image, so it names it again.
  openGraph: { title, description, url: url + path, siteName: "Nuzky", type: "article", locale: "en_US", images: [shareImage("/opengraph-image.jpg")] },
  twitter: { card: "summary_large_image", title, description, images: [shareImage("/twitter-image.jpg")] },
};

const structuredData = {
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "BreadcrumbList",
      itemListElement: [
        { "@type": "ListItem", position: 1, name: "Nuzky", item: url },
        { "@type": "ListItem", position: 2, name: "Nuzky vs CapCut", item: url + path },
      ],
    },
    {
      "@type": "FAQPage",
      mainEntity: capcutFaq.map(({ q, a }) => ({ "@type": "Question", name: q, acceptedAnswer: { "@type": "Answer", text: a } })),
    },
  ],
};

function Picks({ title, items, ours }: { title: string; items: string[]; ours: boolean }) {
  return (
    <div className="surface flex flex-col gap-5 rounded-3xl p-7 sm:p-9">
      <h3 className="text-[22px] font-semibold tracking-[-0.02em]">{title}</h3>
      <ul className="flex flex-col gap-3.5">
        {items.map((item) => (
          <li key={item} className="flex gap-3 text-[15px] leading-[1.45]">
            <Check className={`mt-[3px] size-4 shrink-0 ${ours ? "text-ok" : "text-muted"}`} />
            {item}
          </li>
        ))}
      </ul>
    </div>
  );
}

const link = "underline decoration-white/20 underline-offset-4 transition-colors hover:text-fg";

export default function CapcutAlternative() {
  return (
    <>
      <JsonLd data={structuredData} />
      <Nav base="/" />
      <main>
        <div className="mx-auto w-full max-w-[1240px] px-5">
          <header className="flex flex-col items-center pt-16 text-center sm:pt-24">
            <h1 className="heading-xl">
              The open source
              <br />
              <span className="text-subtle">CapCut alternative.</span>
            </h1>
            <p className="mt-7 max-w-[720px] text-[17px] text-muted sm:text-[20px]">
              Nuzky is a free video editor for Reels, TikTok and Shorts. Auto captions with no limit, 4K export and no watermark, all without a subscription.
            </p>
            <div className="mt-10 flex flex-wrap items-center justify-center gap-3">
              <a href={repo} className="btn-light flex h-12 items-center gap-2 rounded-full px-6 text-[15px] font-semibold">
                <GitHubLogo className="size-[17px]" />
                Get notified on GitHub
              </a>
              <a href="/#editor" className="glass flex h-12 items-center rounded-full px-[22px] text-[15px] font-semibold transition-colors hover:bg-white/10">
                See the editor
              </a>
            </div>
            <p className="mt-3.5 text-[14px] text-muted">Coming soon for macOS, Windows and Linux.</p>

            <Fit width={1200} height={740} className="mt-14 sm:mt-16">
              <div className="relative size-full rounded-[14px] shadow-[0_50px_140px_rgb(0_0_0/0.9),0_0_0_1px_rgb(255_255_255/0.08)]">
                <EditorMock focus="edit" />
              </div>
            </Fit>
          </header>

          <section className="pt-28 sm:pt-[150px]">
            <SectionHeading center first="Nuzky and CapCut, side by side." second="Where each one wins." />
            <div className="surface mx-auto mt-10 max-w-[1000px] overflow-hidden rounded-3xl">
              <table className="w-full border-collapse text-left text-[15px] leading-[1.45]">
                <thead className="max-sm:hidden">
                  <tr className="border-b border-white/[0.07]">
                    <th scope="col" className="w-[22%] px-7 py-5">
                      <span className="sr-only">Compared</span>
                    </th>
                    <th scope="col" className="w-[39%] px-7 py-5 text-[16px] font-semibold">
                      <span className="flex items-center gap-2.5">
                        <Image src="/media/icon.png" alt="" width={24} height={24} />
                        Nuzky
                      </span>
                    </th>
                    <th scope="col" className="w-[39%] px-7 py-5 text-[16px] font-semibold text-muted">
                      CapCut
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {comparison.map((row) => (
                    <tr key={row.label} className="border-b border-white/[0.07] last:border-0 max-sm:block max-sm:px-6 max-sm:py-5">
                      <th
                        scope="row"
                        className="px-7 py-5 align-top text-[13px] font-medium text-muted max-sm:block max-sm:p-0 max-sm:pb-3 max-sm:text-[15px] max-sm:font-semibold max-sm:text-fg"
                      >
                        {row.label}
                      </th>
                      <td className="px-7 py-5 align-top max-sm:block max-sm:p-0 max-sm:pb-2.5">
                        <span className="block text-[12px] text-muted sm:hidden">Nuzky</span>
                        {row.nuzky}
                      </td>
                      <td className="px-7 py-5 align-top text-muted max-sm:block max-sm:p-0">
                        <span className="block text-[12px] text-muted sm:hidden">CapCut</span>
                        {row.capcut}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <div className="mx-auto mt-4 grid max-w-[1000px] gap-4 md:grid-cols-2">
              <Picks title="Pick Nuzky if" items={pickNuzky} ours />
              <Picks title="Stay with CapCut if" items={pickCapcut} ours={false} />
            </div>
          </section>
        </div>

        {/* The light runs edge to edge behind the questions, so their glass has something to bend. */}
        <div className="relative overflow-hidden">
          <Waves swellAt="[data-waves=faq]" intensity={1.9} />
          <section className="relative z-10 mx-auto w-full max-w-[1240px] px-5 pt-28 sm:pt-[150px]">
            <SectionHeading center first="Coming from CapCut?" second="You already know your way around." />
            <div className="mx-auto mt-10 grid max-w-[1000px] gap-4 md:grid-cols-3">
              {fromCapcut.map(({ title, text }) => (
                <div key={title} className="surface flex flex-col gap-2 rounded-3xl p-7">
                  <h3 className="text-[18px] font-semibold tracking-[-0.01em]">{title}</h3>
                  <p className="text-[14px] leading-[1.5] text-muted">{text}</p>
                </div>
              ))}
            </div>
            <FaqList items={capcutFaq} className="mt-16" />
          </section>

          <p className="relative z-10 mx-auto max-w-[760px] px-5 pb-16 pt-28 text-center text-[13px] leading-[1.55] text-muted">
            What this page says about CapCut comes from{" "}
            {capcutSources.map((s, i) => (
              <span key={s.href}>
                {i > 0 && (i === capcutSources.length - 1 ? " and " : ", ")}
                <a href={s.href} rel="nofollow noopener" className={link}>
                  {s.label}
                </a>
              </span>
            ))}
            , checked in {capcutChecked}. CapCut&apos;s plans change, so check them for the current details. CapCut is a trademark of ByteDance. Nuzky is an
            independent project and isn&apos;t affiliated with ByteDance or CapCut.
          </p>
        </div>
      </main>
      <Footer />
    </>
  );
}
