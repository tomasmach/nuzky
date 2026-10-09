import { Agent } from "@/components/Agent";
import { Download } from "@/components/Download";
import { Features } from "@/components/Features";
import { Footer } from "@/components/Footer";
import { Hero } from "@/components/Hero";
import { Nav } from "@/components/Nav";
import { Showcase } from "@/components/Showcase";
import { Steps } from "@/components/Steps";
import { Waves } from "@/components/Waves";

export default function Home() {
  return (
    <>
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
        <Download />
      </main>
      <Footer />
    </>
  );
}
