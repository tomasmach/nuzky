// The pages as plain Markdown for AI agents and LLM search (https://llmstxt.org), built from the same copy as
// the HTML so the two never disagree.

import { type Faq, capcutChecked, capcutFaq, capcutSources, comparison, faq, features, fromCapcut, maker, makerNote, pickCapcut, pickNuzky } from "./content";
import { buildFromSource, license, releaseNotes, repo, url, version } from "./site";

const raw = repo.replace("github.com", "raw.githubusercontent.com") + "/main";
// Links within the site point at the Markdown of the page.
const abs = (href: string) => (href.startsWith("/") ? `${url}${href}.md` : href);
const list = (items: string[]) => items.map((i) => `- ${i}`).join("\n");
const qa = (items: Faq[]) => items.map((f) => `### ${f.q}\n\n${f.a}${f.link ? ` [${f.link.label}](${abs(f.link.href)})` : ""}`).join("\n\n");

const summary =
  "Nuzky is a free, open source desktop video editor in the spirit of CapCut, for TikTok, Reels, Shorts and YouTube. It runs on your computer, writes captions locally in your language and lets an AI agent such as Claude Code or Codex make the rough cut in the open project while you watch.";

const status = `Status: in development. The first version for macOS, Windows and Linux is coming soon. Leave your email at ${url}/#notify for one email when it's out. Current version: ${version}. License: GPL-3.0-or-later.`;

export function homeMarkdown() {
  return `# Nuzky: free, open source video editor

> ${summary}

${status}

No account, no watermark, no subscription and no paid plan coming. Editing, captions and export run on your computer; an AI agent you connect sees the transcript and the frames it checks.

## Why ${maker.name} builds Nuzky

${makerNote.join("\n\n")}

## Features

${list(features)}

## From raw clips to a finished cut

1. Drop in your clips. Nuzky opens video from any phone or camera, including HEVC and variable frame rate.
2. Caption it. AI on your computer writes the captions in your language. Pick a style and fix any word it misheard.
3. Export anywhere. Vertical for Reels and TikTok, widescreen for YouTube, any size up to 4K. The file matches the preview.

## Or hand the cut to ChatGPT or Claude

Connect ChatGPT through Codex or Claude through Claude Code, or any MCP client, and ask for an edit in plain words. It runs on the ChatGPT or Claude plan you already pay for, with no credits to buy: cut the ums and long pauses, add captions, put music under the video, export for Reels. The agent edits the project open in the app. Every change goes through the same undo as yours, your controls lock while it works with a Stop button, and one undo takes the whole run back. Live editing in the app does not work on Windows yet.

## It learns how you edit

Give Nuzky a few raw recordings and the videos you cut from them. It writes down how you edit in an EDIT.md file, with examples from your own videos: which retakes, filler words and pauses you cut, how long your captions are and where they sit, and how often you zoom. Your AI follows it on every new video, and you can change any line.

## Questions

${qa(faq)}

## Links

- [Website](${url})
- [Nuzky vs CapCut](${url}/capcut-alternative)
- [Source code on GitHub](${repo})
- [Release notes](${releaseNotes})
- [Building from source](${buildFromSource})
- [License](${license})
`;
}

export function alternativeMarkdown() {
  const cell = (s: string) => s.replace(/\|/g, "\\|");
  return `# Nuzky vs CapCut: the open source CapCut alternative

> Nuzky is a free, open source video editor for TikTok, Reels, Shorts and YouTube. Auto captions are free with no limit, exports go up to 4K with no watermark, and there is no subscription or account. It does not have CapCut's template library or a phone app.

${status}

## Nuzky and CapCut, side by side

| | Nuzky | CapCut |
| --- | --- | --- |
${comparison.map((r) => `| ${cell(r.label)} | ${cell(r.nuzky)} | ${cell(r.capcut)} |`).join("\n")}

## Pick Nuzky if

${list(pickNuzky)}

## Stay with CapCut if

${list(pickCapcut)}

## Coming from CapCut

${list(fromCapcut.map((f) => `${f.title}: ${f.text}`))}

## Questions from CapCut users

${qa(capcutFaq)}

## Notes

What this page says about CapCut was checked in ${capcutChecked} against ${capcutSources.map((s) => `[${s.label}](${s.href})`).join(", ")}. CapCut's plans change, so check them for the current details. CapCut is a trademark of ByteDance. Nuzky is an independent project and is not affiliated with ByteDance or CapCut.

- [Nuzky home page](${url})
- [Source code on GitHub](${repo})
`;
}

export function llmsTxt() {
  return `# Nuzky

> ${summary}

${status}

No account, no watermark, no subscription. Editing, captions and export run on the computer; an AI agent the user connects sees the transcript and the frames it inspects. It is an alternative to CapCut for creators who edit on a computer. Agents edit video in Nuzky through its MCP server; the agent guide under Source is the one that server serves.

## Pages

- [Nuzky](${url}/index.md): What Nuzky does, its features, how AI agents edit in it, and answers to common questions
- [Nuzky vs CapCut](${url}/capcut-alternative.md): Price, platforms, privacy and AI editing compared, with when each one fits better
- [Everything in one file](${url}/llms-full.txt): Both pages above in a single Markdown file

## Source

- [README](${raw}/README.md): Features, architecture, build steps and the headless CLI
- [Editing with Nuzky for AI agents](${raw}/skills/nuzky-edit/SKILL.md): The MCP workflow for cutting, captioning and exporting a video
- [AI architecture](${raw}/docs/AI-ARCHITECTURE.md): How MCP clients and the in-app AI panel edit a project safely
- [Building from source](${raw}/docs/BUILDING.md): Dependencies and steps for macOS, Windows and Linux

## Optional

- [Behaviour of the editor](${raw}/docs/INTERACTION.md): Every interaction and keyboard shortcut in detail
- [Release notes](${releaseNotes})
- [License](${license}): GPL-3.0-or-later
`;
}

export function llmsFullTxt() {
  return `${homeMarkdown()}\n---\n\n${alternativeMarkdown()}`;
}

// `Vary` keeps a cache from handing the Markdown to a browser, since `/` serves it to agents that ask for
// `text/markdown`. A page's Markdown names its HTML page as canonical, so search engines index only the page;
// llms-full.txt, which holds both pages, is kept out of the index altogether.
// llms.txt files go out as plain text, which every browser shows instead of downloading.
export function markdownResponse(body: string, { canonical, plain = false, noindex = false }: { canonical?: string; plain?: boolean; noindex?: boolean } = {}) {
  const headers: Record<string, string> = { "Content-Type": `text/${plain ? "plain" : "markdown"}; charset=utf-8`, Vary: "Accept" };
  if (canonical) headers.Link = `<${url}${canonical}>; rel="canonical"`;
  if (noindex) headers["X-Robots-Tag"] = "noindex";
  return new Response(body, { headers });
}
