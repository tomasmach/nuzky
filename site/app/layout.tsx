import type { Metadata, Viewport } from "next";
import { Inter, JetBrains_Mono } from "next/font/google";
import { Analytics } from "@vercel/analytics/next";
import { description, url } from "@/lib/site";
import "./globals.css";

// Every character on the page is in the latin subset. The mono face only sets the EDIT.md card far down the
// page, so it loads when that card needs it instead of competing with the hero for the first bytes.
const inter = Inter({ subsets: ["latin"], variable: "--font-inter", display: "swap" });
const mono = JetBrains_Mono({ subsets: ["latin"], variable: "--font-mono-face", display: "swap", preload: false });

// Search results show the title, which names what people search for. Shares show the slogan.
const slogan = "Nuzky | You just wanted to edit. Now you can.";

export const metadata: Metadata = {
  metadataBase: new URL(url),
  title: { default: "Nuzky | Free, open source CapCut alternative", template: "%s | Nuzky" },
  description,
  applicationName: "Nuzky",
  openGraph: { title: slogan, description, url, siteName: "Nuzky", type: "website", locale: "en_US" },
  twitter: { card: "summary_large_image", title: slogan, description },
  robots: { index: true, follow: true, googleBot: { "max-image-preview": "large", "max-snippet": -1, "max-video-preview": -1 } },
};

export const viewport: Viewport = { themeColor: "#08080a", colorScheme: "dark" };

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" className={`${inter.variable} ${mono.variable}`}>
      <body className="min-h-dvh overflow-x-hidden bg-site">
        {children}
        {/* Vercel Web Analytics counts visits without cookies or stored identifiers. */}
        <Analytics />
      </body>
    </html>
  );
}
