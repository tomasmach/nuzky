import path from "node:path";
import type { NextConfig } from "next";

// The site is an npm workspace of the Nuzky repo, so its packages are hoisted to the repo root.
const repoRoot = path.join(__dirname, "..");

// An agent that asks for Markdown first gets the page as Markdown at the page's own URL (see lib/markdown.ts).
// Browsers never put it first, and a client that refuses it with q=0 keeps the HTML. Plain patterns without
// lookahead, since Vercel evaluates these conditions in its own router.
const accept = (value: string) => [{ type: "header" as const, key: "accept", value }];
const markdown = [
  ["/", "/index.md"],
  ["/capcut-alternative", "/capcut-alternative.md"],
].map(([source, destination]) => ({
  source,
  destination,
  has: accept("\\s*text/markdown\\b.*"),
  missing: accept("\\s*text/markdown\\s*;\\s*q=0(\\.0*)?\\s*(,.*)?"),
}));

const config: NextConfig = {
  turbopack: { root: repoRoot },
  outputFileTracingRoot: repoRoot,
  rewrites: async () => ({ beforeFiles: markdown, afterFiles: [], fallback: [] }),
};

export default config;
