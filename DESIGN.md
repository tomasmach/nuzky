# CapOpen design system

Dark only, like Final Cut Pro and CapCut: an editor for video, where the video is the brightest thing on screen. The layout follows CapCut; the finish follows Apple's pro apps. Solid graphite panes sit on a darker window with gaps between them, and the controls that float above them (top bar groups, transport, timeline tools) are glass capsules. Everything is drawn by CSS inside the webview, so it looks the same in WebKitGTK on Linux, WebView2 on Windows and WKWebView on macOS.

## Materials

Defined in `src/index.css` (`@layer components`, so utilities still override them).

| Class | Look | Use |
|---|---|---|
| `pane` | `panel`, 12 px radius, 1 px white 6 % hairline drawn as an inset outline, so content at the edge never covers it | Library, inspector, timeline. The preview pane is `stage` with the same shape |
| `bar` | White 7.5 % fill, white 10 % top highlight and 7 % hairline, soft shadow | Floating capsules: top bar groups, transport, timeline tool groups. Never blurs: it floats over still backgrounds, where a blur would look the same and cost a repaint on every frame below it |
| `overlay` | 82 % graphite, `backdrop-filter: blur(28px) saturate(1.8)`, highlight, deep shadow | Short-lived layers only: menus, popovers, dialogs, toasts, the disabled-reason hint. Falls back to solid `#2a2a2f` where backdrop-filter is missing and with `prefers-contrast: more` |
| `seg-track` / `seg-on` | Recessed white 4.5 % track; the chosen segment white 15 % with a top highlight and a small shadow | Segmented controls, the inspector tabs and the library tabs |
| `btn-prominent` | `accent-strong` fill, white label, top highlight | The one or two prominent actions of a view: Export, Generate captions, the confirm button of a dialog |

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

4 px grid. Gaps between panes 6 px. Radii: panes 12 px, dialogs 20 px, menus and toasts 12–14 px, media tiles 10 px, buttons and inputs 8 px (number fields 6 px), segments 7 px inside a 9 px track, toolbar capsules fully round. Inner radii stay concentric with their container. Icons are lucide at 16 px (14 px inside inspector rows, 11 px inside clips), stroke 2.

Layout widths: library 360 px (Inter is wider than system fonts; at 340 px the tab labels touch), inspector 300 px. The timeline height is user-resizable (160 px up to the space left for a 300 px preview) and remembered; the 6 px gap above it is the resize handle.

## Components

- **Top bar**: on `bg`, no fill of its own. Left: Projects capsule. Centre: project name (13 px semibold) and save state (12 px `muted`). Right: AI run capsule, background job capsule, Connect agent capsule, undo and redo in one capsule, Export as the prominent capsule.
- **Buttons** (`Button`): 32 px high, 8 px radius, 13 px medium. `primary` is `btn-prominent`, `secondary` a white 9 % fill with a hairline, `ghost` text only, `bar` a glass capsule; `pill` makes any of them a capsule. Pressed: 90 % brightness. A trailing ellipsis when the button opens another window or asks more ("Export…").
- **Property row** (`Slider`): label, slider and a typeable value on one line, like CapCut. The value field keeps a draft until Enter or blur, Esc restores, ↑/↓ step.
- **Slider track** (`RangeInput`): 4 px white 13 % track, `accent` fill from the origin to the thumb, a white 22 × 14 px capsule thumb with a small shadow. One-sided ranges fill from the left; two-sided ones (min < 0 < max: temperature, position, rotation) fill from 0 and mark 0 with a white 30 % tick.
- **Number field**: 24 px, white 5.5 % fill, white 8 % border, 6 px radius, right-aligned tabular 12 px; `accent` border while focused.
- **Tabs**: inspector tabs are a full-width segmented control; library tabs are icon over label in a segmented track, sized to their labels so no two labels touch or truncate. The selected library tab's icon is `accent`.
- **Segmented**: small exclusive choices (In/Out, speed presets, export options): `seg-track` with `seg-on` for the chosen segment, 12 px medium labels.
- **Preset tile**: preview on top, 11 px label below, 10 px radius. Selected: accent border plus an `accent-strong` check badge. Caption style tiles show their name in the style itself; a karaoke style lights the last word of the name, or the second half of a one-word name, in its highlight colour.
- **Checkbox**: 16 px, 4 px radius, white 7 % with a white 25 % border; checked or mixed: `accent-strong` with a white check or dash.
- **Track toggles** (hide, mute): the off state swaps the icon (eye-off, speaker-off) and shows `fg` on `raised`. Accent is reserved for selection and "on" features, so a hidden track never looks active. Fixed columns, eye then speaker; a track without one keeps an empty slot so the columns line up.
- **Timeline clips**: 7 px radius, 1 px in from each side so every cut shows, a white 10 % hairline. The name sits in a dark 45 % chip (no blur) at the top left. Selected: 2 px `accent` ring and 8 px `accent` trim handles with a dark grip.
- **Keyframe diamond**: outline when no keyframe sits at the playhead, filled accent when one does. Timeline diamonds are `fg` with a dark border on the selected clip, at mid-height, or along the bottom on the main track so the cut markers never cover them, and clear of the trim handles.
- **Transition marker**: a 20 px rounded square on the cut. Empty: dark with a white 22 % border, "+" on hover. Set: `fg` square with the kind icon, a darkened band shows the transition length; selected: accent.
- **Playhead**: a white 1.5 px line with a rounded white head on the ruler.
- **Video end**: when sound runs past the last picture or text, a dashed 1 px `muted` line marks the end on the ruler and lanes, and the lanes after it lie under black at 30 %.
- **Transport**: one `bar` capsule under the frame: timecode left, Go to start, a white round Play and Go to end in the middle, safe zone, Ratio and full screen right.
- **Preview selection box**: 1.5 px `fg` outline, 9 px square corner handles, a round rotate handle above the top edge. It may extend past the frame but never past the preview area; a handle that would leave the area is pinned to its edge.
- **Toasts**: `overlay`, 14 px radius, bottom-left above the timeline, over the library and no wider than it (336 px), so they never cover the video frame or the transport. While the transcript's Delete bar shows, they sit above it. Info and success toasts close after 5 s (7 s with an action), errors and warnings (`warn` triangle, `warn` border, e.g. why a different project opened) stay until dismissed, and so does a notice of a state that lasts, such as the AI editing. At most four show: a new toast first replaces routine news, never an error or warning; a toast whose Undo no longer applies goes, and an identical one is replaced. Hovering or focusing the stack stops the clock. A focused toast has the 2 px `accent` outline.
- **Menus and popovers**: `overlay`, 12 px radius, 4 px padding, rows 28 px with 6 px radius, hover and keyboard position white 8 %; the current item has an `accent` check.
- **Dialogs**: `overlay`, 20 px radius, 24 px padding, a black 45 % scrim. Title 17 px semibold. Forms align labels right in a column, controls left of a common edge. Buttons are capsules at the bottom right, the confirm one prominent.
- **Font picker**: looks like a select: a white 9 % field, family name at 15 px in its own face, chevron. The list opens below (above when there is no room) with a search field on top; groups "Built in" (each name in its face, 15 px) then "On this computer" (UI font, 13 px), headed like a section at 11 px. The keyboard or pointer position is white 8 %; the current font has an `accent` check. A font missing on this computer shows a `warn` triangle with the reason in the tooltip.
- **Mixed value**: with several clips selected, a number field whose clips differ shows "—"; the slider thumb sits at the first clip's value.
- **Transcript**: 13 px text on a 22 px line, `fg` at 90 %. Each paragraph starts with an 11 px `muted` timecode. Hover is `raised` behind the word; selected words and the spaces between them get `accent` at 30 %, one band; the word playing is `accent` text, black on `accent` inside the selection. Pause chips are 11 px tabular `muted` on `raised`. A corrected word has a dotted `muted` underline; a word being corrected becomes a `raised` field in its place, as wide as its text, with the focus outline. The note that clips are not transcribed carries a `warn` icon. A suggested zoom underlines its sentence, the spaces included, with a 2 px `accent` line at 60 %, and starts with its factor: the 11 px zoom-in icon and "1.24×", 11 px tabular `accent`. The bar of the suggestions sits between the controls and the text, with a white 7 % border below, an `accent` zoom-in icon, the count in 12 px `muted`, Dismiss and the primary Apply.

## States

Focus is a 2 px `accent` outline on every focusable element; never remove it without a replacement. Timeline clips are the one replacement: there the `accent` ring means selected, so a focused clip gets a 2 px `fg` outline 2 px outside it. Hover is a white 8 % fill on icon buttons and rows, white 13–14 % on filled buttons. Disabled controls drop to 40 % opacity and explain why in a tooltip. Disabled buttons, menu items and tiles stay focusable (`aria-disabled`); while one has keyboard focus its reason shows just below it in a 12 px `overlay` box, and screen readers read it as the button's description. Errors use `danger` plus an icon plus text. Background work shows progress; at 0 % it shows an indeterminate bar, which with reduced motion is a full bar of diagonal `accent` stripes, so it never reads as a third done.

## Motion

Buttons and toggles 120 ms ease-out. Dialogs 200 ms. No animation on hover or text changes, with one exception: transition and animation preset tiles show a still frame of the effect and play it in a loop while hovered or focused. Each animation tile rests on the moment that sets it apart (Fade half see-through, Pop at its overshoot, slides half way in), because the motion is the content being chosen. `prefers-reduced-motion` disables all of it.
