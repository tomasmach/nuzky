# CapOpen design system

Dark by default. A dense editor UI: content (the video) is the brightest thing on screen.

## Colour tokens

Defined in `src/index.css` as Tailwind theme variables.

| Token | Value | Use |
|---|---|---|
| `bg` | `#0d0e10` | App background, preview surround, segmented control track |
| `panel` | `#15171a` | Panels |
| `raised` | `#1d2024` | Inputs, cards, hover, toggled-off track buttons |
| `line` | `#2a2e34` | Borders, separators, selected segment |
| `fg` | `#e9ebee` | Primary text, selection box and handles in the preview |
| `muted` | `#9aa1a9` | Secondary text, labels, icons, every informative small text |
| `subtle` | `#656c75` | Disabled text and ruler ticks only; 3.4:1 on `panel` is too low for information |
| `accent` | `#10b981` | Primary action, selection, focus, "on" state |
| `accent-strong` | `#059669` | Primary action hover (brand colour from the icon) |
| `danger` | `#f05252` | Errors, destructive |
| `warn` | `#f5a524` | Warnings, snap guides (timeline and preview), always with a dark 1 px outline so they stay visible on bright footage |

Clip tokens. They always come with an icon and a label, never colour alone. Do not name a token `clip-text`: Tailwind already has `bg-clip-text` (background-clip: text) and the clip would render hollow.

| Token | Value | Clip |
|---|---|---|
| `clip-video` | `#25435a` | Video |
| `clip-image` | `#3a3f63` | Image |
| `clip-audio` | `#1f4a3a` | Audio, including sound detached from a video |
| `clip-title` | `#5a4520` | Text |
| `clip-captions` | `#4a3460` | Captions |

## Type

System UI font. 13 px body, 12 px for dense labels, 11 px minimum for anything informative: ruler labels, badges, clip names and durations, tab labels in the left panel. Timecodes use tabular numerals.

## Shape and spacing

4 px grid. Radius 6 px for controls, 8 px for panels and dialogs. Icons are lucide at 16 px (14 px inside inspector rows, 11 px inside clips).

Layout widths: left panel 340 px, inspector 300 px. The timeline height is user-resizable (160 px up to the space left for a 300 px preview) and remembered.

## Components

- **Property row** (`Slider`): label, slider and a typeable value on one line, like CapCut. The value field keeps a draft until Enter or blur, Esc restores, ↑/↓ step.
- **Slider track** (`RangeInput`): 4 px `line` track, `fg` round thumb, `accent` fill from the origin to the thumb. One-sided ranges fill from the left; two-sided ones (min < 0 < max: temperature, position, rotation) fill from 0 and mark 0 with a `muted` tick.
- **Tabs**: inspector tabs are text with a 2 px accent underline; left panel tabs are icon plus label, sized to their label with a 4 px gap so no two labels touch or truncate.
- **Segmented**: small exclusive choices (In/Out, speed presets, export options). The selected segment is `line` on a `bg` track with medium weight.
- **Preset tile**: preview on top, 11 px label below. Selected: accent border plus a check badge.
- **Checkbox**: dark custom box, accent fill with a black check when on.
- **Track toggles** (hide, mute): the off state swaps the icon (eye-off, speaker-off) and shows `fg` on `raised`. Accent is reserved for selection and "on" features, so a hidden track never looks active. Fixed columns, eye then speaker; a track without one keeps an empty slot so the columns line up.
- **Keyframe diamond**: outline when no keyframe sits at the playhead, filled accent when one does. Timeline diamonds are `fg` with a dark border on the selected clip.
- **Transition marker**: an 18 px square on the cut. Empty: dark with a faint border, "+" on hover. Set: `fg` square with the kind icon, a darkened band shows the transition length; selected: accent.
- **Preview selection box**: 1.5 px `fg` outline, 10 px square corner handles, a round rotate handle above the top edge. It may extend past the frame but never past the preview area; a handle that would leave the area is pinned to its edge.
- **Toasts**: bottom-left above the timeline, over the media panel, so they never cover the video frame. While the transcript's Delete bar shows, they sit above it.
- **Font picker**: looks like a select: `raised` field, family name at 15 px in its own face, chevron. The list opens below (above when there is no room) with a search field on top; groups "Built in" (each name in its face, 15 px) then "On this computer" (UI font, 13 px), headed like a section at 11 px. The keyboard or pointer position is `raised`; the current font has an `accent` check. A font missing on this computer shows a `warn` triangle with the reason in the tooltip.
- **Mixed value**: with several clips selected, a number field whose clips differ shows "—"; the slider thumb sits at the first clip's value.
- **Transcript**: 13 px text on a 22 px line, `fg` at 90 %. Each paragraph starts with an 11 px `muted` timecode. Hover is `raised` behind the word; selected words get `accent` at 30 % behind them; the word playing is `accent` text. Pause chips are 11 px tabular `muted` on `raised`. Out of date, the whole text drops to 40 % and the note above it carries a `warn` icon.

## States

Focus is a 2 px `accent` outline on every focusable element; never remove it without a replacement. Disabled controls drop to 40 % opacity and explain why in a tooltip. Errors use `danger` plus an icon plus text. Background work shows progress; at 0 % it shows an indeterminate bar.

## Motion

Buttons and toggles 120 ms ease-out. Dialogs 200 ms. No animation on hover or text changes, with one exception: transition and animation preset tiles show a still frame of the effect and play it in a loop while hovered or focused. Each animation tile rests on the moment that sets it apart (Fade half see-through, Pop at its overshoot, slides half way in), because the motion is the content being chosen. `prefers-reduced-motion` disables all of it.
