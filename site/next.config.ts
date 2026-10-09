import path from "node:path";
import type { NextConfig } from "next";

// The site is an npm workspace of the Nuzky repo, so its packages are hoisted to the repo root.
const repoRoot = path.join(__dirname, "..");

const config: NextConfig = {
  turbopack: { root: repoRoot },
  outputFileTracingRoot: repoRoot,
};

export default config;
