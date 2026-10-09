import type { Metadata } from "next";
import Image from "next/image";
import { Check } from "lucide-react";
import { Footer } from "@/components/Footer";
import { JsonLd } from "@/components/JsonLd";
import { GitHubLogo } from "@/components/Logos";
import { Nav } from "@/components/Nav";
import { SectionHeading } from "@/components/SectionHeading";
import { capcutChecked, comparison, features, pickCapcut, pickNuzky } from "@/lib/content";
import { repo, url } from "@/lib/site";

const path = "/capcut-alternative";
const title = "Nuzky vs CapCut: the open source CapCut alternative";
const description =
  "Nuzky vs CapCut on price, platforms, privacy and AI. Nuzky is a free, open source video editor for Mac, Windows and Linux with auto captions that run on your computer.";

const shareImage = (src: string) => ({ url: src, width: 2400, height: 1260, alt: "Nuzky: Make the reel. Skip the subscription. A free, open source video editor with AI built in." });

export const metadata: Metadata = {
  title: { absolute: "Open source CapCut alternative for Mac, Windows and Linux | Nuzky" },
  description,
  alternates: { canonical: path, types: { "text/markdown": `${path}.md` } },
  // A page that sets its own Open Graph fields loses the root share image, so it names it again.
  openGraph: { title, description, url: url + path, siteName: "Nuzky", type: "article", locale: "en_US", images: [shareImage("/opengraph-image.jpg")] },
  twitter: { card: "summary_large_image", title, description, images: [shareImage("/twitter-image.jpg")] },
};

const breadcrumbs = {
  "@context": "https://schema.org",
  "@type": "BreadcrumbList",
  itemListElement: [
    { "@type": "ListItem", position: 1, name: "Nuzky", item: url },
    { "@type": "ListItem", position: 2, name: "Nuzky vs CapCut", item: url + path },
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

export default function CapcutAlternative() {
  return (
    <>
      <JsonLd data={breadcrumbs} />
      <Nav base="/" />
      <main className="mx-auto w-full max-w-[1240px] px-5">
        <header className="flex flex-col items-center pt-16 text-center sm:pt-24">
          <h1 className="heading-xl">
            Nuzky vs CapCut.
            <br />
            <span className="text-subtle">Free and open source.</span>
          </h1>
          <p className="mt-7 max-w-[720px] text-[17px] text-muted sm:text-[20px]">
            Nuzky is an open source CapCut alternative for Mac, Windows and Linux. It has the timeline, auto captions and Reels export most people use
            CapCut for, and your footage never leaves your computer.
          </p>
          <div className="mt-10 flex flex-wrap items-center justify-center gap-3">
            <a href="/#editor" className="btn-light flex h-12 items-center rounded-full px-6 text-[15px] font-semibold">
              See the editor
            </a>
            <a href={repo} className="glass flex h-12 items-center gap-2 rounded-full px-[22px] text-[15px] font-semibold transition-colors hover:bg-white/10">
              <GitHubLogo className="size-[17px]" />
              Star on GitHub
            </a>
          </div>
          <p className="mt-3.5 text-[14px] text-muted">Coming soon for macOS, Windows and Linux.</p>
        </header>

        <section className="pt-28 sm:pt-[150px]">
          <SectionHeading center first="At a glance." second={`Checked in ${capcutChecked}.`} />
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
                    <th scope="row" className="px-7 py-5 align-top text-[13px] font-medium text-subtle max-sm:block max-sm:p-0 max-sm:pb-2.5">
                      {row.label}
                    </th>
                    <td className="px-7 py-5 align-top max-sm:block max-sm:p-0 max-sm:pb-2">
                      <span className="block text-[12px] text-subtle sm:hidden">Nuzky</span>
                      {row.nuzky}
                    </td>
                    <td className="px-7 py-5 align-top text-muted max-sm:block max-sm:p-0">
                      <span className="block text-[12px] text-subtle sm:hidden">CapCut</span>
                      {row.capcut}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>

        <section className="pt-28 sm:pt-[150px]">
          <SectionHeading center first="Which one fits?" second="It depends on how you edit." />
          <div className="mx-auto mt-10 grid max-w-[1000px] gap-4 md:grid-cols-2">
            <Picks title="Pick Nuzky if" items={pickNuzky} ours />
            <Picks title="Stay with CapCut if" items={pickCapcut} ours={false} />
          </div>
        </section>

        <section className="pt-28 sm:pt-[150px]">
          <SectionHeading center first="What Nuzky does." second="All of it free." />
          <ul className="mx-auto mt-10 grid max-w-[1000px] gap-x-10 gap-y-3.5 sm:grid-cols-2">
            {features.map((f) => (
              <li key={f} className="flex gap-3 text-[15px] leading-[1.45] text-fg/85">
                <Check className="mt-[3px] size-4 shrink-0 text-ok" />
                {f}
              </li>
            ))}
          </ul>
        </section>

        <p className="mx-auto max-w-[760px] pb-16 pt-28 text-center text-[13px] leading-[1.55] text-subtle">
          What this page says about CapCut was checked against{" "}
          <a href="https://www.capcut.com/pricing" rel="nofollow noopener" className="underline decoration-white/20 underline-offset-4 transition-colors hover:text-muted">
            capcut.com
          </a>{" "}
          in {capcutChecked}. CapCut&apos;s plans change, so check its pricing page for the current details. CapCut is a trademark of ByteDance. Nuzky is an
          independent project and isn&apos;t affiliated with ByteDance or CapCut.
        </p>
      </main>
      <Footer />
    </>
  );
}
