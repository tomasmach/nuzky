import { alternativeMarkdown, markdownResponse } from "@/lib/markdown";

export const dynamic = "force-static";

export function GET() {
  return markdownResponse(alternativeMarkdown(), { canonical: "/capcut-alternative" });
}
