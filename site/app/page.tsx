import { Agent } from "@/components/Agent";
import { Download } from "@/components/Download";
import { Features } from "@/components/Features";
import { Footer } from "@/components/Footer";
import { Hero } from "@/components/Hero";
import { Nav } from "@/components/Nav";
import { Showcase } from "@/components/Showcase";
import { Steps } from "@/components/Steps";

export default function Home() {
  return (
    <>
      <Nav />
      <main>
        <Hero />
        <Showcase />
        <Steps />
        <Agent />
        <Features />
        <Download />
      </main>
      <Footer />
    </>
  );
}
