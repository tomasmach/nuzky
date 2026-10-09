import path from "node:path";
import type { NextConfig } from "next";

// The site is an npm workspace of the Nuzky repo, so its packages are hoisted to the repo root.
const repoRoot = path.join(__dirname, "..");

// An agent that asks for Markdown gets the page as Markdown at the page's own URL (see lib/markdown.ts).
const markdown = [
  ["/", "/index.md"],
  ["/capcut-alternative", "/capcut-alternative.md"],
].map(([source, destination]) => ({
  source,
  destination,
  has: [{ type: "header" as const, key: "accept", value: "(.*)text/markdown(.*)" }],
}));

const config: NextConfig = {
  turbopack: { root: repoRoot },
  outputFileTracingRoot: repoRoot,
  rewrites: async () => ({ beforeFiles: markdown, afterFiles: [], fallback: [] }),
};

export default config;
