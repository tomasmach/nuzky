import type { Metadata } from "next";
import { Agent } from "@/components/Agent";
import { Download } from "@/components/Download";
import { Faq } from "@/components/Faq";
import { Features } from "@/components/Features";
import { Footer } from "@/components/Footer";
import { Hero } from "@/components/Hero";
import { JsonLd } from "@/components/JsonLd";
import { Nav } from "@/components/Nav";
import { Showcase } from "@/components/Showcase";
import { Steps } from "@/components/Steps";
import { Waves } from "@/components/Waves";
import { faq, features } from "@/lib/content";
import { description, releaseNotes, repo, url, version } from "@/lib/site";

export const metadata: Metadata = {
  alternates: { canonical: "/", types: { "text/markdown": "/index.md" } },
};

// No rating or download URL until real ones exist: made-up ratings break Google's structured data rules.
const structuredData = {
  "@context": "https://schema.org",
  "@graph": [
    { "@type": "WebSite", "@id": `${url}/#website`, url, name: "Nuzky", description, inLanguage: "en" },
    {
      "@type": "SoftwareApplication",
      "@id": `${url}/#app`,
      name: "Nuzky",
      url,
      description,
      image: `${url}/media/icon.png`,
      applicationCategory: "MultimediaApplication",
      applicationSubCategory: "Video editor",
      operatingSystem: "macOS, Windows, Linux",
      softwareVersion: version,
      releaseNotes,
      license: "https://www.gnu.org/licenses/gpl-3.0.html",
      isAccessibleForFree: true,
      offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
      featureList: features,
      sameAs: [repo],
    },
    {
      "@type": "FAQPage",
      mainEntity: faq.map(({ q, a }) => ({ "@type": "Question", name: q, acceptedAnswer: { "@type": "Answer", text: a } })),
    },
  ],
};

export default function Home() {
  return (
    <>
      <JsonLd data={structuredData} />
      <Nav />
      <main>
        {/* One light surface behind the hero and the editor tour, so the two never meet in a seam. */}
        <div id="top" className="relative -mt-[72px] overflow-hidden pt-[72px]">
          <Waves swellAt="[data-waves=swell]" glowAt="[data-waves=glow]" height={0.75} intensity={1.4} />
          <Hero />
          <Showcase />
        </div>
        <Steps />
        <Agent />
        <Features />
        <Faq />
        <Download />
      </main>
      <Footer />
    </>
  );
}
