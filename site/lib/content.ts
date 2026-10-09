// Copy that the pages, the structured data for search engines and the Markdown for AI agents all share,
// so a fact changes in one place.

export type Faq = { q: string; a: string; link?: { label: string; href: string } };

export const faq: Faq[] = [
  {
    q: "Is Nuzky really free?",
    a: "Yes. There is no paid plan, no account and no watermark on your exports. Nuzky is open source under GPL-3.0, so anyone can read, build and share the code. AI editing runs on your own Claude Code or Codex plan.",
  },
  {
    q: "When can I download Nuzky?",
    a: "Nuzky is still in development. The first builds for macOS, Windows and Linux are coming soon. On GitHub, choose Watch, then Custom, then Releases, and GitHub emails you when the first build ships.",
  },
  {
    q: "How is Nuzky different from CapCut?",
    a: "Nuzky is open source, free with no account, and runs on Linux too. It has a timeline, auto captions, caption styles and an export made for Reels and TikTok. It doesn't have CapCut's library of templates and effects, and there is no phone app.",
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
    a: "Yes. Connect Claude Code or Codex in one click, or any other MCP client, and ask for a cut in plain words. You watch every change in the open project, you can stop it at any time, and one undo takes the whole run back. The agent runs on your own Claude Code or Codex plan, and live editing in the app doesn't work on Windows yet.",
  },
  {
    q: "Which files can Nuzky open and export?",
    a: "Video, audio and images from phones and cameras, including HEVC and H.264, variable frame rate and front camera footage. It exports MP4 (H.264 and AAC) from 720p to 4K. The Reels and TikTok preset writes 1080 × 1920 at 30 fps with the sound levelled to −14 LUFS.",
  },
];

export const features = [
  "Vertical 9:16 canvas, plus 16:9, 1:1 and 4:5",
  "Opens phone and camera footage, including HEVC, H.264, variable frame rate, rotated and front camera video",
  "Magnetic main track with overlay video, audio and text tracks",
  "Trim, split, move and delete with snapping, undo and redo",
  "Edit by text: delete words in the transcript and the video cuts with them",
  "Keyframes, speed, entry and exit animations, transitions and filter presets",
  "Text with outline, background box, colour, position, rotation and opacity",
  "Auto captions with Whisper on your computer, seven caption styles and a highlight on the spoken word",
  "Clean voice for speech recorded with room noise or hum",
  "Real-time preview with sound",
  "MP4 export (H.264 and AAC) from 720p to 4K, drawn by the same renderer as the preview",
  "Reels and TikTok preset: 1080 × 1920 at 30 fps, sound levelled to −14 LUFS",
  "Saves after every edit and recovers after a crash",
  "AI agents edit the open project through MCP: Claude Code, Codex or any MCP client, with one undo per run (not on Windows yet)",
  "Learns how a creator edits from their finished cuts into an EDIT.md that agents follow",
];

// What the comparison page says about CapCut was checked against capcut.com/pricing, CapCut's help centre
// and its online editor page. Recheck it when this date gets old.
export const capcutChecked = "October 2026";
export const capcutSources = [
  { label: "CapCut pricing", href: "https://www.capcut.com/pricing" },
  { label: "CapCut help on Pro features", href: "https://www.capcut.com/help/capcut-pro-prompt-standard-user" },
  { label: "CapCut online editor", href: "https://www.capcut.com/tools/online-video-editor" },
];

export const comparison: { label: string; nuzky: string; capcut: string }[] = [
  {
    label: "Price",
    nuzky: "Free. No paid plan, no account, no watermark. AI editing uses your own Claude Code or Codex plan.",
    capcut: "Free plan with basic tools. Premium templates, Pro features and AI credits need the paid Pro or Ultra plan.",
  },
  {
    label: "Auto captions",
    nuzky: "Free and on your computer, with seven styles and a highlight on the spoken word",
    capcut: "Include Pro-only parts, according to CapCut's help centre",
  },
  { label: "Export", nuzky: "MP4 from 720p to 4K at 24 to 60 fps. No watermark.", capcut: "A video that uses any Pro feature needs Pro to export" },
  { label: "Source code", nuzky: "Open source, GPL-3.0", capcut: "Closed source, made by ByteDance" },
  {
    label: "Computers",
    nuzky: "Linux first. macOS and Windows builds are coming and not tested yet.",
    capcut: "macOS and Windows apps. On Linux, only the web editor, which needs an account.",
  },
  { label: "Phones", nuzky: "No phone app", capcut: "iOS and Android" },
  {
    label: "Your footage",
    nuzky: "Edited, captioned and exported on your computer. An AI agent you connect sees the transcript and the frames it checks.",
    capcut: "The phone and desktop apps edit files on your device. In the web editor you upload your clips to CapCut.",
  },
  {
    label: "AI editing",
    nuzky: "Your own agent edits the open project: Claude Code, Codex or any MCP client. One undo takes a run back. Not on Windows yet.",
    capcut: "Built-in AI tools, paid for with monthly credits",
  },
  {
    label: "Templates and effects",
    nuzky: "No template library. Seven filter presets, eight transitions, entry and exit animations and seven caption styles.",
    capcut: "A large library of templates, effects and stickers",
  },
  { label: "Status", nuzky: "In development, first builds coming soon", capcut: "Mature and widely used" },
];

export const pickNuzky = [
  "You edit on a computer, including Linux",
  "You don't want another subscription",
  "You want editing and captions to run on your computer",
  "You already use Claude Code or Codex and want it to do the rough cut",
  "You want to read, change or build the code yourself",
];

export const pickCapcut = [
  "You edit on your phone",
  "You build videos from templates, effects and stickers",
  "You need your projects synced across devices",
  "You need a finished, released app today",
];

export const fromCapcut = [
  { title: "The same layout", text: "Media on the left, the preview in the middle, the inspector on the right and the timeline across the bottom." },
  { title: "Q and W", text: "Delete the part of a clip left or right of the playhead, CapCut's fast way to cut a talking head." },
  { title: "Edit by text", text: "Delete words in the transcript and the video cuts with them, with captions and overlays kept in sync." },
];

export const capcutFaq: Faq[] = [
  {
    q: "Can I open my CapCut projects in Nuzky?",
    a: "No. Bring the original clips and rebuild the cut. CapCut templates and stickers don't carry over.",
  },
  {
    q: "Is there a CapCut app for Linux?",
    a: "No. CapCut's web editor runs in a browser on Linux, but it needs an account and you upload your clips to it. Nuzky is built and tested on Linux first.",
  },
  {
    q: "How is Nuzky different from Kdenlive or Shotcut?",
    a: "Kdenlive and Shotcut are general-purpose editors. Nuzky is made for short vertical videos: it's built around a 9:16 canvas, styles captions with a highlight on the spoken word, has a Reels and TikTok preset and lets an AI agent make the cut.",
  },
];
