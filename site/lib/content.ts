// Copy that the pages, the structured data for search engines and the Markdown for AI agents all share,
// so a fact changes in one place. It is written for creators who edit reels, not for developers.

export type Faq = { q: string; a: string; link?: { label: string; href: string } };

export const faq: Faq[] = [
  {
    q: "Is Nuzky really free?",
    a: "Yes. No subscription, no watermark on your videos and no account to make. Nuzky is open source, so it stays free. If you let AI do the editing, it runs on your own Claude or ChatGPT plan.",
  },
  {
    q: "When can I download it?",
    a: "Soon. The first version for Mac, Windows and Linux is on its way. Watch Nuzky on GitHub to hear the moment it's out.",
  },
  {
    q: "How is Nuzky different from CapCut?",
    a: "Everything in Nuzky is free: auto captions, 4K export, no watermark and no account. It's made for editing on your computer. CapCut has more templates and effects, and a phone app.",
    link: { label: "Compare Nuzky and CapCut", href: "/capcut-alternative" },
  },
  {
    q: "Are the auto captions free?",
    a: "Yes, with no limit. Nuzky writes captions from what you say, right on your computer. Pick a style, make the spoken word light up and fix any word it got wrong.",
  },
  {
    q: "Are my videos private?",
    a: "Yes. Your videos stay on your computer, and nothing gets uploaded to edit, caption or export them. If you ask an AI to edit for you, it sees what's said in the video and the frames it checks.",
  },
  {
    q: "Can AI edit the video for me?",
    a: "Yes. Say what you want, like “cut the ums and pauses and add captions”, and watch it edit in the app. You can stop it any time, and one undo takes it all back. It works through your own Claude or ChatGPT plan, and not on Windows yet.",
  },
  {
    q: "Will videos from my phone work?",
    a: "Yes. iPhone and Android videos open the right way up, selfies included, and every cut lands on the exact frame you picked.",
  },
  {
    q: "Is it ready for TikTok, Reels and Shorts?",
    a: "Yes. One click exports a vertical 1080 × 1920 video with the volume levelled for social apps.",
  },
];

// For search engines and AI agents, which want the full picture.
export const features = [
  "Vertical 9:16 canvas, plus 16:9, 1:1 and 4:5",
  "Opens phone and camera footage, including HEVC, H.264, variable frame rate, rotated and front camera video",
  "Magnetic main track with overlay video, audio and text tracks",
  "Trim, split, move and delete with snapping, undo and redo",
  "Edit by text: delete words in the transcript and the video cuts with them",
  "Keyframes, speed, entry and exit animations, transitions and filter presets",
  "Text with outline, background box, colour, position, rotation and opacity",
  "Auto captions on your computer with no limit, seven caption styles and a highlight on the spoken word",
  "Clean voice for speech recorded with room noise or hum",
  "Real-time preview with sound",
  "MP4 export from 720p to 4K with no watermark, drawn by the same renderer as the preview",
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
  { label: "Price", nuzky: "Free. No subscription.", capcut: "Free plan, then Pro or Ultra for premium features and AI credits" },
  { label: "Auto captions", nuzky: "Free, with no limit", capcut: "Some caption features need Pro" },
  { label: "Export", nuzky: "Up to 4K, never a watermark", capcut: "Videos that use Pro features need Pro to export" },
  {
    label: "Your videos",
    nuzky: "Stay on your computer. An AI you ask to edit sees what's said and the frames it checks.",
    capcut: "The apps edit on your device. The web editor uploads them.",
  },
  {
    label: "AI editing",
    nuzky: "Tell it what to cut and watch it work. Uses your Claude or ChatGPT plan. Not on Windows yet.",
    capcut: "Built-in AI tools, paid with credits",
  },
  {
    label: "Templates and effects",
    nuzky: "Filters, transitions, animations and seven caption styles. No template library.",
    capcut: "A large library of templates, effects and stickers",
  },
  { label: "Where it runs", nuzky: "Mac, Windows and Linux", capcut: "Phones, Mac, Windows and the web" },
  { label: "Open source", nuzky: "Yes, free for good", capcut: "No" },
  { label: "Available", nuzky: "Coming soon", capcut: "Now" },
];

export const pickNuzky = [
  "You're done paying a subscription to edit",
  "You want free auto captions with no limit",
  "You want your videos to stay on your computer",
  "You'd like AI to do the boring first cut",
];

export const pickCapcut = [
  "You edit on your phone",
  "You build videos from templates, effects and stickers",
  "You need your projects on every device",
  "You need an app you can download today",
];

export const fromCapcut = [
  { title: "The same layout", text: "Media on the left, the preview in the middle, settings on the right and the timeline at the bottom." },
  { title: "Q and W", text: "Cut away the part before or after the playhead with one key, just like in CapCut." },
  { title: "Edit by text", text: "Delete words in the transcript and the video cuts with them, captions included." },
];

export const capcutFaq: Faq[] = [
  {
    q: "Can I open my CapCut projects in Nuzky?",
    a: "No. Bring your original clips and edit them again. CapCut templates and stickers don't come along.",
  },
  {
    q: "Are CapCut's auto captions free?",
    a: "Not entirely. CapCut's help centre says auto captions include features that need Pro. In Nuzky, captions are free with no limit.",
  },
  {
    q: "Does Nuzky get any rights to my videos?",
    a: "No. Nuzky doesn't upload your videos and asks for no rights to anything you make.",
  },
];
