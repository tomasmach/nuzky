import type { Metadata, Viewport } from "next";
import { Inter, JetBrains_Mono } from "next/font/google";
import "./globals.css";

const inter = Inter({ subsets: ["latin", "latin-ext"], variable: "--font-inter", display: "swap" });
const mono = JetBrains_Mono({ subsets: ["latin"], variable: "--font-mono-face", display: "swap" });

const description = "A free, open source video editor with AI built in. Edit reels, TikToks and everything else on your own computer.";

export const metadata: Metadata = {
  metadataBase: new URL("https://nuzky.app"),
  title: "Nuzky — Make the reel. Skip the subscription.",
  description,
  openGraph: { title: "Nuzky", description, url: "https://nuzky.app", siteName: "Nuzky", type: "website" },
  twitter: { card: "summary_large_image", title: "Nuzky", description },
};

export const viewport: Viewport = { themeColor: "#08080a", colorScheme: "dark" };

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" className={`${inter.variable} ${mono.variable}`}>
      <body className="min-h-dvh overflow-x-hidden bg-site">{children}</body>
    </html>
  );
}
