// Copy that the pages, the structured data for search engines and the Markdown for AI agents all share,
// so a fact changes in one place.

export type Faq = { q: string; a: string; link?: { label: string; href: string } };

export const faq: Faq[] = [
  {
    q: "Is Nuzky really free?",
    a: "Yes. Every feature is free, with no paid plan, no account and no watermark on your exports. Nuzky is open source under GPL-3.0, so it stays free.",
  },
  {
    q: "Is Nuzky a good CapCut alternative?",
    a: "If you edit short videos on a computer, yes. Nuzky has a timeline, auto captions, caption styles and an export made for Reels and TikTok. It doesn't have CapCut's library of templates and effects, and there is no phone app.",
    link: { label: "Compare Nuzky and CapCut", href: "/capcut-alternative" },
  },
  {
    q: "Does Nuzky run on Linux?",
    a: "Yes. Linux is where Nuzky is built and tested first. Builds for macOS, Windows and Linux are coming soon, and you can build it from source today.",
  },
  {
    q: "Do my videos get uploaded anywhere?",
    a: "No. Editing, captions and export all run on your computer. Nuzky only goes online to download its speech models and to check for a new version once a day, and you can turn that check off. An AI agent you connect sees the transcript and the frames it checks, like anything else you share with it.",
  },
  {
    q: "How do the auto captions work?",
    a: "Whisper turns the speech into captions on your computer, and music and silence stay uncaptioned. Each line lands on the timeline as a clip. Pick one of seven styles, such as Reel, Karaoke or Box, highlight the spoken word and fix any word it misheard.",
  },
  {
    q: "Can an AI agent edit my video?",
    a: "Yes. Connect Claude Code or Codex in one click, or any other MCP client, and ask for a cut in plain words. You watch every change in the open project, you can stop it at any time, and one undo takes the whole run back.",
  },
  {
    q: "Which files can Nuzky open and export?",
    a: "Video, audio and images from phones and cameras, including HEVC and H.264, variable frame rate and front camera footage. It exports MP4 (H.264 and AAC). The Reels and TikTok preset writes 1080 × 1920 at 30 fps with the sound levelled to −14 LUFS.",
  },
  {
    q: "When can I download Nuzky?",
    a: "Nuzky is still in development. The first builds for macOS, Windows and Linux are coming soon. Follow the project on GitHub to hear when they ship.",
  },
];

export const features = [
  "Vertical 9:16 canvas, plus 16:9, 1:1 and 4:5",
  "Opens phone and camera footage, including HEVC, H.264, variable frame rate, rotated and front camera video",
  "Magnetic main track with overlay video, audio and text tracks",
  "Trim, split, move and delete with snapping, undo and redo",
  "Keyframes, speed, entry and exit animations, transitions and filter presets",
  "Text with outline, background box, colour, position, rotation and opacity",
  "Auto captions with Whisper on your computer, caption styles and a highlight on the spoken word",
  "Clean voice for speech recorded with room noise or hum",
  "Real-time preview with sound",
  "MP4 export (H.264 and AAC) drawn by the same renderer as the preview",
  "Reels and TikTok preset: 1080 × 1920 at 30 fps, sound levelled to −14 LUFS",
  "Saves after every edit and recovers after a crash",
  "AI agents edit the open project through MCP: Claude Code, Codex or any MCP client, with one undo per run",
  "Learns how a creator edits from their finished cuts into an EDIT.md that agents follow",
];

// What the comparison page says about CapCut was checked against capcut.com/pricing and CapCut's list of
// platforms. Recheck it when this date gets old.
export const capcutChecked = "October 2026";

export const comparison: { label: string; nuzky: string; capcut: string }[] = [
  {
    label: "Price",
    nuzky: "Free. Every feature, no paid plan, no account and no watermark.",
    capcut: "Free plan with basic tools. Premium templates, Pro features and AI credits need the paid Pro or Ultra plan.",
  },
  { label: "Source code", nuzky: "Open source, GPL-3.0", capcut: "Closed source, made by ByteDance" },
  { label: "Computers", nuzky: "macOS, Windows and Linux", capcut: "macOS and Windows. No Linux app." },
  { label: "Phones", nuzky: "No phone app", capcut: "iOS and Android" },
  {
    label: "Your footage",
    nuzky: "Stays on your computer. Captions run locally with Whisper.",
    capcut: "Paid plans add cloud storage, and AI features run on credits.",
  },
  {
    label: "AI editing",
    nuzky: "Your own agent edits the open project: Claude Code, Codex or any MCP client. One undo takes a run back.",
    capcut: "Built-in AI tools, paid for with monthly credits",
  },
  { label: "Templates and effects", nuzky: "Not yet", capcut: "A large library of templates, effects and stickers" },
  { label: "Status", nuzky: "In development, first builds coming soon", capcut: "Mature and widely used" },
];

export const pickNuzky = [
  "You edit on a Mac, a Windows PC or Linux",
  "You don't want another subscription",
  "Your footage should stay on your computer",
  "You want an AI agent to do the rough cut while you watch",
  "You want to read, change or build the code yourself",
];

export const pickCapcut = [
  "You edit on your phone",
  "You build videos from templates, effects and stickers",
  "You need your projects synced across devices",
  "You need a finished, released app today",
];
