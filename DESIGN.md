# CapOpen design system

Dark by default. A dense editor UI: content (the video) is the brightest thing on screen.

## Colour tokens

Defined in `src/index.css` as Tailwind theme variables.

| Token | Value | Use |
|---|---|---|
| `bg` | `#0d0e10` | App background, preview surround |
| `panel` | `#15171a` | Panels |
| `raised` | `#1d2024` | Inputs, cards, hover |
| `line` | `#2a2e34` | Borders, separators |
| `fg` | `#e9ebee` | Primary text |
| `muted` | `#9aa1a9` | Secondary text, icons |
| `subtle` | `#656c75` | Disabled text, ticks |
| `accent` | `#10b981` | Primary action, selection, focus |
| `accent-strong` | `#059669` | Primary action hover (brand colour from the icon) |
| `danger` | `#f05252` | Errors, destructive |
| `warn` | `#f5a524` | Warnings |

Clip colours always come with an icon and a label, never colour alone:
video `#25435a`, image `#3a3f63`, audio `#1f4a3a`, text `#5a4520`, captions `#4a3460`.

## Type

System UI font. 13 px body, 12 px for dense labels, 11 px for ruler ticks. Timecodes use tabular numerals.

## Shape and spacing

4 px grid. Radius 6 px for controls, 8 px for panels and dialogs. Icons are lucide at 16 px.

## States

Focus is a 2 px `accent` ring on every focusable element. Disabled controls drop to `subtle` and explain why in a tooltip. Errors use `danger` plus an icon plus text.

## Motion

Buttons and toggles 120 ms ease-out. Dialogs 200 ms. No animation on hover or text changes. `prefers-reduced-motion` disables transitions.
