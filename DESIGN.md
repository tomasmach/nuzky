# Nuzky design system

Dark only, like Final Cut Pro and CapCut: an editor for video, where the video is the brightest thing on screen. The layout follows CapCut; the finish follows Apple's pro apps. Solid graphite panes sit on a darker window with gaps between them, and the controls that float above them (top bar groups, transport, timeline tools) are glass capsules. Everything is drawn by CSS inside the webview, so it looks the same in WebKitGTK on Linux, WebView2 on Windows and WKWebView on macOS.

This file is the law for how Nuzky looks; [docs/INTERACTION.md](docs/INTERACTION.md) is the law for how it behaves. A change to either goes into the same PR as the code.

## What Nuzky should feel like

Someone who edits in CapCut opens Nuzky and finds everything where they expect it. Then it feels calmer and more finished, as if Apple had made it: quiet graphite, crisp type, precise edges, nothing loud. Phone videos are the content, and the interface steps back around them.

- **The video is the brightest, most colourful thing on screen.** The chrome is graphite and monochrome. Colour carries meaning only: blue for selection, focus, "on" and the prominent action, green for success and sound, orange for warnings, red for danger, and the muted clip colours on the timeline.
- **CapCut's layout, unchanged.** Library with icon tabs on the left, preview in the middle with the transport under it, inspector on the right, timeline across the bottom, Export at the top right. Things do not move between versions; people and AI agents rely on where they are.
- **Two layers, as in Apple's design.** Solid panes are the content layer. The controls that float above them (top bar groups, transport, timeline tools) and the short-lived overlays (menus, dialogs, toasts) are the functional layer. Glass belongs only to that layer.
- **Restraint.** One prominent blue action per view, two at most. Hierarchy comes from size, weight and space, not from boxes, badges or colour. No helper text that repeats a label; a short line only where it prevents a mistake.
- **Precision.** Edges line up (the top bar capsules line up with the panes below them), inner radii are concentric with their container, numbers are tabular, spacing follows the 4 px grid. Every control is finished in all its states: hover, pressed, focus, disabled, locked while the AI edits, empty, loading and error.
- **Smooth on a weak PC.** Effects have a budget (see Materials). Nothing repaints on every frame unless it has to.
- **The same on every platform.** Drawn in CSS inside the webview with the bundled Inter, never with native vibrancy or system fonts that differ per platform.

Never: glow, gradients on the chrome, gradient text, emoji, walls of badges, uppercase eyebrows over sections, decorative illustrations, a light theme or a theme switch, animation on hover, blur on repeated items or over playing video.

## Working on the UI

- Before changing a screen, read the parts of this file it touches and the flow in docs/INTERACTION.md. Build from the shared components in `src/components/ui.tsx` and the materials in `src/index.css`; extend them, do not copy them.
- Compute the contrast of every new colour from its hex values: 4.5:1 for text up to 17 px, 3:1 for icons and larger text.
- Look at the change in the real app: screenshots from `scripts/repro.py` at 1440 × 900 and at the smallest window, 1024 × 640. Check the empty, disabled, locked (AI editing) and error states, not only the happy path.
- When a change touches playback, the timeline, dragging or anything repeated per clip, measure before and after (frame times, renders per scroll, React time per playback frame) and give the numbers.
- After a larger UI change, have an independent reviewer critique screenshots of the real app against this file.
- For design reviews and questions of convention, Apple's Human Interface Guidelines are the reference; the `apple-design` agent skill carries them with a review checklist, where an agent has it. Read them as a desktop app in a webview: dark only is right for an app built around video (HIG Dark Mode, "immersive media viewing"); 13 px body text, 11 px minimum here (the HIG allows 10 px on macOS); controls 28 px or more, never under 20 px; every action also reachable from the keyboard; glass only on the floating functional layer.

## Platform notes

Found while building this design; keep them in mind before reaching for an effect.

- WebKitGTK left the preview canvas black when its box had a blurred `box-shadow`. The frame keeps square corners and a 1 px hairline.
- `@starting-style` transitions left the dialog scrim transparent in WebKitGTK. Entry animations use `@keyframes`.
- Rounded `outline`s need WebKitGTK 2.40 or newer and Chromium 94 or newer; pane hairlines rely on them.
- WebKitGTK draws `<input type="color">` like a switch, so the colour field draws its own swatch over a see-through input.
- WebKitGTK sets Inter a little wider than Chromium. Leave slack in tight rows such as the library tabs.
- `prefers-reduced-transparency` does not exist in WebKitGTK; `prefers-contrast: more` turns overlays opaque instead.
- Tailwind 4 orders utilities of the same property by value, so `className="h-6"` cannot shrink a component whose base size is also a utility. The shared components keep their base shapes in `@layer components` classes (`.btn`, `.icon-btn`, `.pane`…), which any utility overrides.

## Materials

Defined in `src/index.css` (`@layer components`, so utilities still override them).

| Class | Look | Use |
|---|---|---|
| `pane` | `panel`, 12 px radius, 1 px white 6 % hairline drawn as an inset outline, so content at the edge never covers it | Library, inspector, timeline. The preview pane is `stage` with the same shape |
| `bar` | White 7.5 % fill, white 10 % top highlight and 7 % hairline, soft shadow | Floating capsules: top bar groups, transport, timeline tool groups. Never blurs: it floats over still backgrounds, where a blur would look the same and cost a repaint on every frame below it |
| `overlay` | 88 % graphite, `backdrop-filter: blur(28px) saturate(1.8)`, highlight, deep shadow | Short-lived layers only: menus, popovers, dialogs, toasts, the disabled-reason hint. Falls back to solid `#2a2a2f` where backdrop-filter is missing and with `prefers-contrast: more` |
| `overlay-thick` | With `overlay`: 96 % graphite and `saturate(1.1)`, so little of the picture's colour comes through | An overlay that is mostly text over the video: the launcher |
| `seg-track` / `seg-on` | Recessed white 4.5 % track; the chosen segment white 15 % with a top highlight and a small shadow | Segmented controls, the inspector tabs and the library tabs |
| `btn-prominent` | `accent-strong` fill, white label, top highlight; disabled, a quiet white 8 % capsule with 40 % text | The one or two prominent actions of a view: Export, Generate captions, the confirm button of a dialog |

Performance rules: `backdrop-filter` only on `overlay`, never on anything that lives over the preview canvas or over the timeline while it scrolls or plays. While the video plays, `overlay` drops its blur and turns solid (`html[data-playing]`), since a menu or dialog opened during playback would otherwise re-blur every frame. No blur, filter or large shadow on repeated items (clips, media tiles, transcript words). No `transition: all`; transitions on colour, opacity or transform only. The preview frame has square corners and only a 1 px hairline: a rounded clip would mask the canvas on every frame, and in WebKitGTK a blurred shadow on the frame left the canvas black.

## Colour tokens

Tailwind theme variables in `src/index.css`. Contrast figures are for 13 px text.

| Token | Value | Use |
|---|---|---|
| `bg` | `#0c0c0e` | Window background, the gaps between panes, behind the top bar |
| `stage` | `#121214` | Preview pane around the frame |
| `panel` | `#1c1c1f` | Panes |
| `raised` | `#2c2c30` | Solid inputs, hover rows in lists, cards |
| `line` | `#36363b` | Borders and separators (inside panes prefer white 7 %) |
| `fg` | `#f2f2f5` | Primary text, selection box and handles in the preview |
| `muted` | `#a1a1a9` | Secondary text, labels, icons, every informative small text (6.6:1 on `panel`, 5.4:1 on `raised`) |
| `subtle` | `#6e6e75` | Disabled text and ruler ticks only; 3.4:1 on `panel` is too low for information |
| `accent` | `#2997ff` | Selection, focus, slider fill, "on" state, links and text actions (5.6:1 on `panel`, 4.6:1 on `raised`) |
| `accent-strong` | `#0071e3` | Prominent button fill and checked checkboxes, always with white (4.7:1) |
| `ok` | `#30d158` | Success icons |
| `danger` | `#ff453a` | Errors, destructive |
| `warn` | `#ff9f0a` | Warnings, snap guides (timeline and preview), always with a dark 1 px outline so they stay visible on bright footage |

Never put white text on `accent` (2.9:1); text on `accent` is black, buttons use `accent-strong`.

Clip tokens. They always come with an icon and a label, never colour alone; caption clips show only their text, as the Captions track header carries the icon. Do not name a token `clip-text`: Tailwind already has `bg-clip-text` (background-clip: text) and the clip would render hollow.

| Token | Value | Clip |
|---|---|---|
| `clip-video` | `#20466d` | Video |
| `clip-image` | `#3a3d72` | Image |
| `clip-audio` | `#144a31` | Audio, including sound detached from a video |
| `clip-title` | `#6a4314` | Text |
| `clip-captions` | `#4a3478` | Captions |

## Type

Inter (`Inter UI`, the variable file in `assets/fonts/inter`, optical sizes on) for the whole UI, the nearest open face to SF Pro. 13 px body, 12 px for labels and dense controls, 11 px minimum for anything informative: ruler labels, badges, clip names and durations, library tab labels. Weights 400, 500 for controls and labels in capsules, 600 for titles; never light weights. Section titles are 13 px semibold `fg`, sentence case, no uppercase eyebrows. The inspector header is 15 px semibold, dialog titles 17 px semibold. Timecodes and every number that changes use tabular numerals.

## Shape and spacing

The root font size is 16 px, so Tailwind's rem sizes are the px values here (`h-8` is 32 px); body text is set to 13 px on `body`. 4 px grid. Gaps between panes 6 px. Radii: panes 12 px, dialogs 20 px, menus and toasts 12–14 px, media tiles 10 px, buttons and inputs 8 px (number fields 6 px), segments 7 px inside a 9 px track, toolbar capsules fully round. Inner radii stay concentric with their container. Icons are lucide at 16 px (14 px inside inspector rows, 11 px inside clips), stroke 2.

Layout: the library and the inspector start at 28 % of the editor's width and the timeline at 35 % of the window's height, close to CapCut's proportions, and follow the window until someone resizes them. The 6 px gaps around the preview and above the timeline are their resize handles, with a short grip (white 30 % on hover, `accent` while dragging). Sizes are remembered. The library is at least 360 px (Inter is wider than system fonts; at 340 px the tab labels touch), the inspector 300 px and the timeline 160 px; each can grow until the preview has 340 px left (the narrowest at which the transport still shows the current time in full) and 300 px of height. Grids in the library add columns as it widens instead of growing their tiles.

## Components

- **Top bar**: 48 px on `bg`, no fill of its own. Left: Projects capsule, which opens the launcher, and while a newer version is out a round `bar` button with an `accent` arrow-down circle that opens the updates menu. Centre: project name (13 px semibold) with the save state (12 px `muted`) hanging off its right edge, so a changing status never moves the name. Right: AI run capsule (a still accent icon, no pulse), background job capsule, AI toggle capsule (white 15 % while the panel is open; a spinning `accent` loader instead of the sparkles while its agent works with the panel closed), undo, redo and versions in one capsule, Export as the prominent capsule. The versions menu is a menu of rows: an `accent` check on the current version, a `muted` sparkle for an AI run, the label (cut with an ellipsis when long) and the time on the right in 12 px `muted`.
- **Buttons** (`Button`): 32 px high, 8 px radius, 13 px medium. `primary` is `btn-prominent`, `secondary` a white 9 % fill with a hairline, `ghost` text only, `bar` a glass capsule; `pill` makes any of them a capsule. Pressed: 90 % brightness. A trailing ellipsis when the button opens another window or asks more ("Export…").
- **Property row** (`Slider`): label, slider and a typeable value on one line, like CapCut. The value field keeps a draft until Enter or blur, Esc restores, ↑/↓ step.
- **Slider track** (`RangeInput`): 4 px white 13 % track, `accent` fill from the origin to the thumb, a white 22 × 14 px capsule thumb with a small shadow. One-sided ranges fill from the left; two-sided ones (min < 0 < max: temperature, position, rotation) fill from 0 and mark 0 with a white 30 % tick.
- **Number field**: 24 px, white 5.5 % fill, white 8 % border, 6 px radius, right-aligned tabular 12 px; `accent` border while focused.
- **Colour field**: hex in 12 px `muted` and a 32 × 20 px swatch with a white 18 % hairline, drawn over a see-through colour input (WebKitGTK draws the native one like a switch).
- **Tabs**: inspector tabs are a full-width segmented control; library tabs are icon over label in a segmented track, sized to their labels so no two labels touch or truncate. The selected library tab's icon is `accent`.
- **Segmented**: small exclusive choices (In/Out, speed presets, export options): `seg-track` with `seg-on` for the chosen segment, 12 px medium labels.
- **Preset tile**: preview on top, 11 px label below, 10 px radius. Selected: accent border plus an `accent-strong` check badge. Caption style tiles show their name in the style itself; a karaoke style lights the last word of the name, or the second half of a one-word name, in its highlight colour.
- **Checkbox**: 16 px, 4 px radius, white 7 % with a white 25 % border; checked or mixed: `accent-strong` with a white check or dash.
- **Track toggles** (hide, mute): the off state swaps the icon (eye-off, speaker-off) and shows `fg` on `raised`. Accent is reserved for selection and "on" features, so a hidden track never looks active. Fixed columns, eye then speaker; a track without one keeps an empty slot so the columns line up.
- **Timeline tools**: split, delete left, delete right, delete and duplicate in one `bar` capsule, then a divider and the snapping toggle; zoom on the right.
- **Timeline clips**: 7 px radius, 1 px in from each side so every cut shows, a white 10 % hairline. The name sits in a dark 60 % chip (no blur) at the top left, so it stays readable over bright footage. Selected: 2 px `accent` ring and 8 px `accent` trim handles with a dark grip.
- **Keyframe diamond**: outline when no keyframe sits at the playhead, filled accent when one does. Timeline diamonds are `fg` with a dark border on the selected clip, at mid-height, or along the bottom on the main track so the cut markers never cover them, and clear of the trim handles.
- **Transition marker**: a 20 px rounded square on the cut. Empty: dark with a white 22 % border, "+" on hover. Set: `fg` square with the kind icon, a darkened band shows the transition length; selected: accent.
- **Playhead**: a white 1.5 px line with a rounded white head on the ruler.
- **Video end**: when sound runs past the last picture or text, a dashed 1 px `muted` line marks the end on the ruler and lanes, and the lanes after it lie under black at 30 %.
- **Transport**: one 44 px `bar` capsule under the frame: timecode left, Go to start, a white round Play and Go to end in the middle, safe zone and Ratio right. When the preview pane is under 460 px wide (the smallest windows), the total length and the word Ratio hide, so the current time always shows.
- **Preview selection box**: 1.5 px `fg` outline, 9 px square corner handles, a round rotate handle above the top edge. On videos and images an 18 × 5 px `fg` bar with the handles' dark border sits along the middle of each edge for cropping; while cropping, a dashed 1 px `fg` 60 % outline shows the whole picture. The box outlines what is visible. It may extend past the frame but never past the preview area; a handle that would leave the area is pinned to its edge.
- **Toasts**: `overlay`, 14 px radius, bottom-left above the timeline, over the library and no wider than it (336 px), so they never cover the video frame or the transport. While the transcript's Delete bar shows, they sit above it. Info and success toasts close after 5 s (7 s with an action), errors and warnings (`warn` triangle, `warn` border, e.g. why a different project opened) stay until dismissed, and so does a notice of a state that lasts, such as the AI editing. At most four show: a new toast first replaces routine news, never an error or warning; a toast whose Undo no longer applies goes, and an identical one is replaced. Hovering or focusing the stack stops the clock. A focused toast has the 2 px `accent` outline.
- **Menus and popovers**: `overlay`, 12 px radius, 4 px padding, rows 28 px with 6 px radius, hover and keyboard position white 8 %; the current item has an `accent` check.
- **Dialogs**: `overlay`, 20 px radius, 24 px padding, a black 45 % scrim; both fade in over 200 ms (keyframes). Title 17 px semibold. Forms align labels right in a column, controls left of a common edge. Buttons are capsules at the bottom right, the confirm one prominent.
- **Home screen**: the editor's frame without its panes: the same 48 px top bar (left a back capsule with the open project's name, right the AI run, job and AI capsules; AI shows the spinning loader while the panel's agent works) over a 232 px sidebar pane and the main pane, 6 px apart. The main pane's header is 64 px: the title at 17 px semibold with the count in 13 px `muted`, then the search field (300 px, shrinking to 180 px; hidden on the first start), Sort as a ghost button, Open file as an icon button and New project as the one prominent button, each opening a menu. Toasts sit at the grid's left edge, clear of the sidebar. Section titles are 13 px semibold `fg`. Empty states are one centred block (icon, 15 px semibold title, one 13 px `muted` line), never a frame.
- **Project card**: the picture box is `stage` with a 10 px radius and the pane hairline, 152 × 180 in shape so a 9:16 video fills its height; the picture keeps its canvas shape inside it with square corners, like the preview frame. Cards are 148–168 px wide in a grid, 16 px apart, 20 px between rows, so a small window does not get larger cards. Name 13 px medium `fg`, then 12 px `muted` (`danger` for a damaged file) for when and the collection. Chips in the picture are the timeline's dark 60 % chips at 11 px: the length bottom right, `AI` with an `accent` sparkle or "2 missing" with a `warn` triangle bottom left, "In use" with a lock in the middle of a picture at 35 %. Hover and keyboard focus show a round check top left and a … button top right (dark 55 %); hover lightens the hairline to white 16 %; focus, and an open card menu, are the 2 px `accent` outline 2 px outside the box; selected is a 2 px `accent` ring and a filled `accent-strong` check. While the list loads, white 4 % boxes and text bars hold the cards' places. An empty project is a dashed white 15 % shape of its format with a film icon; one that cannot be read shows a `danger` alert icon and "Can't open". While a picture loads, its shape shows at white 4 %, so nothing moves when it arrives.
- **Version row**: pinned to the foot of the home sidebar, below the scrolling collections, a sidebar row without an icon: “Nuzky 0.1.0” in 12 px `muted`. With an update it stands out as a 36 px row on `accent` 16 % with an `accent` 45 % inner line: an `accent` arrow-down circle, “Update available” 13 px semibold `fg` and the version in 12 px `accent` where the count goes; while a check the user asked for runs, a `muted` spinner and “Checking for updates…”. Its menu opens upwards.
- **Sidebar rows**: 32 px, 8 px radius, a 16 px icon, the name and a 12 px tabular `muted` count. Hover white 5 %; the current one white 8 % with a medium label and an `accent` icon; a collection under dragged cards `accent` at 20 % with an `accent` hairline. A collection is named in place in a white 5 % row with the focus outline.
- **Launcher**: an `overlay overlay-thick` 680 px wide (at most the window less 24 px each side), 20 px radius, 10 px padding, 88 px from the top over a black 45 % scrim, fading in like a dialog. A 48 px search field (white 6 %, white 8 % border, `accent` while focused, 17 px text). Rows are 48 px with a 10 px radius: a 36 px picture, name 13 px medium, a 12 px `muted` line, the time on the right; the row the keyboard is on is white 8 % with a ↵ key cap. Group titles are 12 px semibold `muted`. Below a white 8 % rule, the New row of small secondary capsules (format shape and ratio), then the key hints at 11 px. The question about an AI run replaces that row in a white 5 % panel, and no row is marked then, as Enter answers the question. Its Undo changes button is the `danger` style: it throws the agent's work away. As a dialog on the home screen the question has the 17 px dialog title.
- **Search results**: a quote of what was said is 12 px `muted` with the found words in `fg` on `accent` at 25 %, 3 px radius.
- **Media tiles**: square, 10 px radius; the duration sits bottom-right in a small dark 60 % chip. Hover and focus show 26 px round actions along the bottom: `+` prominent, then picture in picture (videos and images) and remove as dark 55 % circles with a white 18 % hairline. An empty library shows one centred invitation (icon, "No media yet", the drop hint and the Import tile), not a frame around it.
- **AI panel**: a `pane`, 360 px wide by default (as wide as the inspector in the inspector's place, where it runs down beside the timeline). Header 48 px with a white 7 % line under it: a 15 px grip (`muted`, a 24 × 28 px button), the agent's name at 13 px semibold with a chevron, then New chat and Close as 28 px icon buttons. Messages at 13 px on a 20 px line: the user's in a `raised` bubble on the right, 14 px radius with the bottom-right corner 4 px, the chips it carried under it in 11 px `muted`; the agent's as plain text at `fg` 90 % with its Markdown drawn (lists, bold, inline code on white 8 %), never as HTML. Steps sit between two white 6 % lines, 26 px rows of 12 px text: a 14 px status icon (`accent` spinner, `ok` check, `danger` alert, `muted` stop), the title, the detail right-aligned in tabular `muted`, cut with an ellipsis when long. Choices are `raised` 10 px rounded buttons, label 13 px medium and detail 12 px `muted`; the chosen one is `accent` 18 % with an `accent` 60 % inner line and a check, the rest drop to 50 %. The run card is `raised`, 10 px radius, 12 px padding: an `ok` check (a `muted` stop when stopped, a `muted` undo arrow once undone), the label 13 px semibold, Undo as a small secondary button, and the changes as 12 px rows starting at the label's edge, with the time right-aligned in tabular `muted`; a row that leads somewhere gets white 6 % on hover, and an undone card's rows drop to `fg` 45 %. Problems use the same card with a `danger`, `warn` or `accent` (information) icon. The field is a white 5.5 % box with a white 8 % border, 12 px radius, `accent` border while its text has focus (its buttons keep their own focus ring); chips 22 px, white 8 %, 6 px radius, × on the right; the picture toggle a 28 px icon button (`accent` 18 % when on); Send a 28 px round `accent-strong` button with a white arrow, a quiet white 8 % circle when it cannot send; while working, a `muted` spinner with the elapsed time and a secondary Stop. Docked, the 6 px gap beside it is the resize handle with the timeline divider's grip. Floating, it keeps the pane look with a white 12 % outline and a 50 px black 55 % shadow, no blur, so it costs nothing over the playing video. The drop preview while moving it is a 2 px `accent` outline on `accent` 10 % with 12 px radius and the place's name in a black 60 % chip.
- **Font picker**: looks like a select: a white 9 % field, family name at 15 px in its own face, chevron. The list opens below (above when there is no room) with a search field on top; groups "Built in" (each name in its face, 15 px) then "On this computer" (UI font, 13 px), headed like a section at 11 px. The keyboard or pointer position is white 8 %; the current font has an `accent` check. A font missing on this computer shows a `warn` triangle with the reason in the tooltip.
- **Mixed value**: with several clips selected, a number field whose clips differ shows "—"; the slider thumb sits at the first clip's value.
- **Transcript**: 13 px text on a 22 px line, `fg` at 90 %. Each paragraph starts with an 11 px `muted` timecode. Hover is `raised` behind the word; selected words and the spaces between them get `accent` at 30 %, one band; the word playing is `accent` text, black on `accent` inside the selection. Pause chips are 11 px tabular `muted` on `raised`. A corrected word has a dotted `muted` underline; a word being corrected becomes a `raised` field in its place, as wide as its text, with the focus outline. The note that clips are not transcribed carries a `warn` icon. A suggested zoom underlines its sentence, the spaces included, with a 2 px `accent` line at 60 %, and starts with its factor: the 11 px zoom-in icon and "1.24×", 11 px tabular `accent`. The bar of the suggestions sits between the controls and the text, with a white 7 % border below, an `accent` zoom-in icon, the count in 12 px `muted`, Dismiss and the primary Apply.

## States

Focus is a 2 px `accent` outline on every focusable element; never remove it without a replacement. Timeline clips are one replacement: there the `accent` ring means selected, so a focused clip gets a 2 px `fg` outline 2 px outside it. Resize gaps are the other: a ring would run along the edges of the panes, so a focused gap shows its grip in `accent`. Hover is a white 8 % fill on icon buttons and rows, white 13–14 % on filled buttons. Disabled controls drop to 40 % opacity and explain why in a tooltip. Under the AI lock, sliders and number fields look read-only instead (labels stay `muted`, values `fg` at 80 % without a field, a grey thumb), so the values the agent changes stay readable. Scrollbar thumbs show only while the pointer is over the scroller. Disabled buttons, menu items and tiles stay focusable (`aria-disabled`); while one has keyboard focus its reason shows just below it in a 12 px `overlay` box, and screen readers read it as the button's description. Errors use `danger` plus an icon plus text. Background work shows progress; at 0 % it shows an indeterminate bar, which with reduced motion is a full bar of diagonal `accent` stripes, so it never reads as a third done.

## Motion

Buttons and toggles 120 ms ease-out. Dialogs 200 ms. No animation on hover or text changes, with one exception: transition and animation preset tiles show a still frame of the effect and play it in a loop while hovered or focused. Each animation tile rests on the moment that sets it apart (Fade half see-through, Pop at its overshoot, slides half way in), because the motion is the content being chosen. `prefers-reduced-motion` disables all of it.

## App icon

A pair of satin titanium scissors with polished edges and a polished pivot screw, slightly open, on a near-black body lit softly from the top: a precision tool, as monochrome as the chrome of the app. No colour, no text and no picture of the app's UI. It sits inside macOS's rounded square with its margin and soft shadow, so the same artwork works on the Dock, a Linux launcher and the Windows taskbar. The 1024 px master is `assets/icon.png`; `npx tauri icon assets/icon.png` regenerates `src-tauri/icons` (delete the `android` and `ios` folders it adds).
