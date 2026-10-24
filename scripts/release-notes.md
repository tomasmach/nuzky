The first alpha of Nuzky, a free, open-source video editor for your computer. It's early, so expect rough edges. Nuzky never changes or deletes your original videos.

## What it does

- **Captions in Czech and English.** Speech is recognised on your computer, with no limit. Pick a style and make the spoken word light up. The first run downloads the speech model.
- **Retakes and pauses.** Transcribe a talking video and delete words straight from the text, or press Remove retakes and pauses to cut restarted sentences, filler words and silences in one step. One Undo takes it back.
- **Export.** MP4 from 720p to 4K, with presets for Reels & TikTok, Shorts and YouTube and no watermark. The file looks exactly like the preview.
- **Phone videos.** iPhone and Android videos open the right way up, selfies included, and every cut lands on the frame you picked.
- **AI editing with ChatGPT, Claude Code or Codex.** Sign in with ChatGPT in the AI panel, no terminal needed, or connect Claude Code or Codex in one click. You watch it edit, Stop works at any time, and one Undo takes back the whole run. It runs on the plan you already have. Gemini works from a terminal through Google's Antigravity CLI, and the AI panel shows how.
- **Your CapCut drafts.** The home screen lists the drafts CapCut keeps on your computer. Open one and its clips, cuts, speed, volume, keyframes, texts, captions and transitions come over. A list says what didn't, and the draft itself stays as it was.
- **Reels from a long video.** AI finds the moments that stand on their own, you trim each one by its words, and Nuzky makes a vertical project for every reel.
- **Covers and thumbnails.** A 9:16 cover and a 16:9 thumbnail from a frame of your video, with your text behind you, saved as PNG or JPEG.
- **Music and sound.** Paste a YouTube or YouTube Music link to add its sound, search CC0 and CC BY music and effects, or use the effects built in. Nuzky lists the credits to copy.
- **Picture and voice.** Blur or replace the background behind a person, keep a face in frame when you change the shape, and clean up a voice recorded in a room.
- Nuzky saves after every edit and recovers after a crash.

## Not yet

- Intel Macs, Linux on ARM and Windows on ARM aren't supported.
- Export makes MP4 only. There's no template library and no phone app.

## Systems

macOS works and is where Nuzky is tested. Linux is only partly tested. Windows isn't tested at all: the installer builds, but nobody has tried it on a Windows PC yet. If something breaks on Linux or Windows, please report it.

- **macOS 14 or newer on Apple Silicon**: the `.dmg`. Open it and drag Nuzky to Applications.
- **Windows 10 or 11, x64**: the `-setup.exe`. The installer downloads Microsoft WebView2 if it's missing.
- **Linux, x86_64**: the `.deb` for Ubuntu 24.04 or newer (`sudo apt install ./Nuzky_*.deb`), or the `.AppImage` for other distributions with glibc 2.39 or newer. Make the AppImage executable with `chmod +x` and run it.

## Opening an unsigned build

- **macOS**: Apple hasn't checked this build, so macOS won't open it the first time. Close the message, open System Settings → Privacy & Security, click Open Anyway next to Nuzky and confirm.
- **Windows**: Nuzky has no Microsoft signing certificate yet, so SmartScreen says "Windows protected your PC" the first time. Click More info, then Run anyway.

## Updates

Once a day Nuzky reads the version number of the latest release on this page, and nothing else. When a newer one is out, the version at the bottom of the home screen's sidebar says so. Click it and choose Update: Nuzky downloads the new version from this page and keeps it only if its signature matches the key built into Nuzky. When you click Restart to update, it saves your project, installs the new version and restarts. It never restarts by itself, and while an export or another job runs, Restart to update waits for it. Nothing downloads or installs before you click. To turn the check off, untick Check automatically in the same menu.

## Found a bug?

Open an issue at https://github.com/nuzkyapp/nuzky/issues, or use Send feedback → Report a bug… in the app, which fills in your version and system. Say what you did and what happened, and please don't attach private videos. No GitHub account? Reply to a post or send a DM to [@mach_builds](https://x.com/mach_builds) on X.

## Source

Nuzky is GPL-3.0-or-later, and its source is the Source code archive below. The installers include FFmpeg, ONNX Runtime and other libraries. Their source, and what each installer was built from, is in [Third-party source for Nuzky v0.1.0-alpha](https://github.com/nuzkyapp/nuzky/releases/tag/third-party-source-0.1.0-alpha).
