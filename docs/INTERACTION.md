# Interaction spec

How the editor behaves. Visual tokens live in [DESIGN.md](../DESIGN.md).

## Layout

Top bar (project, save state, canvas format, undo/redo, Export) · left panel (Media, Text, Captions) · preview · inspector · timeline across the bottom. The layout follows CapCut so its users find everything where they expect it.

## Response time

| Action | Feedback |
|---|---|
| Click, toggle, select | Visual state change in the same frame |
| Seek or scrub | Last frame stays on screen until the new one arrives, so the preview never flashes |
| Import | Item appears at once with a skeleton thumbnail; audio preparation shows a thin progress bar on the item |
| Export, captions | Background job with percentage and phase; the user keeps editing; a toast reports the result |

## Flows

### Import
Entry: Import button, `Ctrl+I`, or files dropped on the window. Files dropped on the timeline are also placed on it.
Outcome: items appear in Media. Unsupported files produce one toast naming each file and the reason; the rest still import.

### Add to timeline
`+` on a media item adds it at the playhead (video and images on the main track, audio on an audio track). Dragging an item onto a track places it at the pointer. The new clip is selected.

### Edit clips
- **Select**: click. `Shift`-click adds to the selection. Click empty space or press `Esc` to clear.
- **Move**: drag the clip body. A ghost follows the pointer and snaps within 8 px to the playhead and clip edges; a vertical line shows the snap. Dropping on an occupied spot creates a new track. `Esc` cancels the drag.
- **Main track**: magnetic. Clips sit back to back; moving reorders, deleting closes the gap.
- **Trim**: drag a clip edge. A tooltip shows the new duration. The handle stops at the end of the source media.
- **Split**: `S` or the Split button, at the playhead. Disabled with the reason when no selected clip is under the playhead.
- **Delete**: `Delete` or `Backspace`. A toast offers Undo.
- **Undo/redo**: `Ctrl+Z`, `Ctrl+Shift+Z` or `Ctrl+Y`. A slider drag or a typing burst is one step.

### Playback
`Space` plays and pauses. Clicking or dragging the ruler scrubs. `←`/`→` step one frame, with `Shift` one second. `Home`/`End` jump to the ends. The sound card drives the clock, so picture and sound stay in sync.

### Inspector
Shows the selected clip. With nothing selected it shows project settings (canvas format and background). Changes preview live.

### Text and captions
Text presets add a text clip at the playhead and select it. Captions: choose language, model and style, then Generate. The first run downloads the model with progress. Recognition runs in the background and can be cancelled. Generating again replaces the previous Captions track, and the button label says so.

### Export
`Ctrl+E` or Export opens a dialog with format, resolution and frame rate, then asks where to save. Progress shows percentage and frame count, with Cancel. Success offers Show in folder. Failure keeps the dialog open with the reason and Retry. Export is disabled with a reason while the timeline is empty.

### Saving
Every edit is saved about a second later. The top bar shows Saving…, Saved, or a failure with an icon and text.

## Keyboard

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `S` | Split selected clip at playhead |
| `Delete`, `Backspace` | Delete selection |
| `Ctrl+Z` / `Ctrl+Shift+Z`, `Ctrl+Y` | Undo / redo |
| `←` `→` (`Shift`) | Step a frame (a second) |
| `Home` / `End` | Start / end |
| `Ctrl` + wheel, `+` / `-` | Zoom timeline |
| `Ctrl+I` | Import |
| `Ctrl+E` | Export |
| `Esc` | Cancel drag, clear selection, close dialog |

Shortcuts are ignored while typing in a field.
