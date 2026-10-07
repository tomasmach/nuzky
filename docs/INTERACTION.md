# Interaction spec

How the editor behaves. Visual tokens live in [DESIGN.md](../DESIGN.md).

## Layout

Top bar (projects, name, save state, background jobs, undo/redo, Export) · left panel (Media, Audio, Text, Captions, Transcript, Transitions, Filters) · preview with transport and Ratio · inspector · timeline across the bottom. The layout follows CapCut so its users find everything where they expect it. Each control exists once: the canvas format lives in Ratio under the preview, the timecode in the transport.

Drag the line above the timeline to resize it; double-click resets it, ↑/↓ on the focused handle resize by 24 px. The height is remembered.

## Response time

| Action | Feedback |
|---|---|
| Click, toggle, select | Visual state change in the same frame |
| Seek or scrub | Last frame stays on screen until the new one arrives, so the preview never flashes |
| Import | Item appears at once with a skeleton thumbnail; audio preparation shows a thin progress bar on the item |
| Export, captions, transcript | Background job with percentage and phase in the top bar; the user keeps editing; a toast reports the result |

## Flows

### Import
Entry: Import button, `Ctrl+I`, or files dropped on the window. Files dropped on the timeline are also placed on it; while media files are dragged over the timeline, an `accent` line shows where they would land. Each picked or dropped file shows at once in Media (and Audio for sound files) with its name and a skeleton thumbnail while it is read. The Audio tab's "Add music or sound" opens the picker filtered to audio.
Outcome: items appear in Media (all kinds) and, for audio, in Audio. Unsupported files produce one toast naming each file by name only with a short reason ("build.log is not a video, audio or image file"); the rest still import.
Hovering or focusing a media or audio item shows `+` (add at playhead) and a trash button (remove the item and its clips at once; a toast offers Undo, like deleting clips).

### Add to timeline
`+` on a media item adds it at the playhead (video and images on the main track, audio on an audio track). Dragging an item onto a track places it at the pointer. The new clip is selected.

### Edit clips
- **Select**: click. `Shift`-click adds to the selection. Click empty space or press `Esc` to clear.
- **Keyboard**: the clips are one tab stop (the clip focused last, else the first selected, else the first on the main track). On a focused clip `←`/`→` go to the previous or next clip on its track and `↑`/`↓` to the nearest clip on the track above or below, without moving the playhead, and select the clip they reach, so `Delete`, `S`, `Q`, `W` and `Ctrl+D` act on the clip with focus. `Ctrl` with an arrow moves focus without selecting, to build a selection with `Shift+Enter` (adds or removes the focused clip); `Enter` selects only it. `Alt+←`/`Alt+→` nudge it a frame (`Shift`: a second), stopping at its neighbours; on the main track they trade places with the clip beside it. A burst of nudges is one undo step. The Menu key or `Shift+F10` opens the clip's context menu; closing the menu returns focus to the clip. When the focused clip is deleted, focus and selection move to the clip that took its place on the track, else the one before it. `Space` works as everywhere.
- **Move**: drag the clip body. A ghost follows the pointer and snaps within 8 px to the playhead and clip edges; a vertical line shows the snap. Dropping on an occupied spot creates a new track. `Esc` cancels the drag, and so does the system taking the pointer or the window losing focus.
- **Main track**: magnetic. Clips sit back to back; moving reorders, deleting closes the gap. While a clip is dragged the track shows the result live: the neighbours move aside, a dashed slot and an `accent` bar mark where it goes in, and the gap it left closes. Trimming a main-track clip keeps its start and moves the clips after it with the edge. Media dragged over the main track marks the cut it would go in at.
- **Trim**: drag a clip edge. A tooltip shows the new duration. The handle stops at the end of the source media, taking the clip's speed into account.
- **Split**: at the playhead. `S` and the Split button split the selected clips under the playhead, else the main-track clip under it, and are disabled with the reason when there is none; the context menu splits the selected clips under the playhead.
- **Cuts sound clean**: where two pieces of sound meet that do not play on from each other, they crossfade over 20 ms around the cut (10 ms on each side, reaching past the clips where the files have sound), so no cut clicks or dips. Split halves that play on, and cuts with a transition, are left as they are.
- **Delete left / right**: `Q` and `W` or the buttons next to Split remove the part of the clip left or right of the playhead, CapCut's fast way to cut a talking head. They act on the selected clips under the playhead, else the main-track clip under it. On the main track the time is cut from every track, so captions, titles and overlays stay in sync; audio tracks set to keep in place (see Transcript) are left alone. A clip on another track is trimmed instead. After `Q` the playhead sits on the cut. Each press is one undo step.
- **Duplicate**: `Ctrl+D`, the Duplicate button or the context menu. The copy goes right after the original and is selected.
- **Delete**: `Delete`, `Backspace`, the toolbar button or the context menu; all of them show a toast with Undo. With a transition selected they remove the transition.
- **Context menu**: right-click a clip for Split, Duplicate, Detach audio, Select all on track and Delete. Right-clicking a clip outside the selection selects it first; every item then acts on the selection, and with several clips selected the labels say how many (“Delete 3 clips”). Select all on track uses the clicked clip's track. Unavailable items are disabled with the reason in a tooltip; ↑/↓ reach them too and show the reason. ↑/↓ move, Esc closes.
- **Undo/redo**: `Ctrl+Z`, `Ctrl+Shift+Z` or `Ctrl+Y`. A slider drag, a typing burst, a preview drag or a caption restyle is one step. Undo in a toast undoes the step the toast names; once another change follows it, the toast no longer offers Undo.
- **Order**: edits, undo and redo apply one at a time in the order they were made. An edit made before the previous one is confirmed builds on it, so two quick changes (two sliders, two keyframes) both stick.

### Timeline display
Clips are drawn 1 px in from each side so every cut is visible. Video clips show a filmstrip whose frames match the source position under each tile, speed included (a single repeated thumbnail until the filmstrip is ready), and a thin waveform strip at the bottom while they have sound. Clips not at 1x carry a speed badge ("2x"). Thin bars at the top edge show the length of the entry (left) and exit (right) animation. The selected clip shows its keyframes as diamonds; clicking one, or `Enter` on a focused one, moves the playhead there. Caption clips show only their text. When sound runs past the last picture or text, the video ends there: a dashed line on the ruler and lanes marks the end, the time after it is dimmed, and the ruler's tooltip says the sound after it is not exported.

### Transitions
Every cut on the main track has a small square. Clicking an empty one adds a 0.5 s Dissolve and selects it; clicking a set one selects it. A selected transition opens in the inspector with its name, duration and Remove; the kind is picked in the Transitions tab. The Transitions tab applies to the selected cut, else the cut before the selected main-track clip, else the cut nearest the playhead, and names that cut. Duration is capped at 2 s and by the shorter neighbouring clip. None removes the transition.

### Filters
The Filters tab applies a preset (None, Vivid, Warm, Cool, Mono, Fade, Moody, Punch) to the selected video or image clip. A preset replaces the clip's Adjust values; the matching preset is marked. Without a media clip selected the tiles are disabled with the reason.

### Playback
`Space` plays and pauses; the focused control keeps focus and is not pressed. A focused checkbox or menu item keeps Space for itself. Clicking or dragging the ruler scrubs. `←`/`→` step one frame, with `Shift` one second. `Home`/`End` jump to the ends. A focused slider keeps its own arrow, Home and End keys. The sound card drives the clock, so picture and sound stay in sync.

### Preview
Clicking the paused preview selects the top-most visible layer under the pointer; clicking where no layer is clears the selection. The selected layer gets a box: drag inside it to move, drag a corner to scale uniformly, drag the round handle to rotate (`Shift` snaps to 15°). The box of a layer larger than the frame is cut off at the edge of the preview area, and handles that would leave it stay on that edge, so they can always be grabbed. While moving, the layer centre snaps to the canvas centre lines within 6 px and a guide shows. Each gesture is one undo step. If the clip has keyframes, the gesture writes the keyframe at the playhead. Ratio, right of the transport, switches the canvas between 9:16, 16:9, 1:1 and 4:5; in its menu ↑/↓ move, and choosing or Esc returns focus to Ratio.

### Inspector
Shows the selected clip with CapCut's tabs, remembering the last tab per clip kind:

| Selection | Tabs |
|---|---|
| Video clip | Video (transform), Adjust, Speed, Animation, Audio (only when the file has sound) |
| Image clip | Image (transform), Adjust, Animation |
| Text or caption clip | Text, Animation, Transform |
| Audio clip (also detached sound) | Audio: volume, fades, speed |
| Transition | Kind name, duration, Remove |
| Nothing | Project: format (opens Ratio) and frame rate, background colour or blur with strength |
| Several clips | Duplicate, Delete, and the controls they all share: Transform, Font (text), Adjust (video, images), Audio volume (clips with sound) |

Adjust offers Exposure, Brightness, Contrast, Highlights, Shadows, Saturation, Temperature, Tint, Fade, and Vignette for single or multiple media clips.

Number fields keep what you type until Enter or blur, so "-" or "1," is never rejected mid-typing; Esc restores the value, ↑/↓ step (`Shift` ×10), commas work as decimal points. Changes preview live.

- **Keyframes**: the diamond in the Transform header adds a keyframe with the current values at the playhead, or removes the one there (filled diamond). Arrows jump to the previous and next keyframe. Once a clip has keyframes, every transform change updates or creates the keyframe at the playhead. Removing the last keyframe keeps its values. Reset clears the transform and all keyframes.
- **Zoom over clip**: a punch-in or pull-out in one step. Apply writes a keyframe at the start and one at the end of the clip, from the first scale to the second, keeping the position and rotation at the playhead. The fields start at the current scale and 20 points more, or at the first and last keyframe when there are some. Existing keyframes are replaced, with a toast that offers Undo.
- **Several clips**: one change applies to every selected clip as one undo step, a slider drag included. Where the clips differ the field shows "—"; a typed value then applies to all. A transform change applies to the whole clip, every keyframe included, so "110 % on every clip" is Select all on track plus one value.
- **Speed**: presets 0.5x–3x or a logarithmic slider from 0.1x to 10x. Speed changes the clip length; the tab shows the new duration and the source length used.
- **Animation**: In and Out each take one preset with a duration; the selected preset is marked and the slot label shows a dot when set.
- **Audio**: volume, fade in and fade out (up to half the clip). Detach audio moves the sound to its own audio track and silences the video clip; disabled with the reason when there is nothing to detach.

### Text and captions
Text presets add a text clip at the playhead, centred in the frame, and select it; captions sit a little below the middle (y +0.15, where reels put them). Transform Reset puts each back there. The inspector offers the same style presets under the same names as the Text tab, or the Captions tab for a caption. The inspector calls a caption clip "Caption".

**Font**: the Font field opens a searchable list. Fonts that ship with CapOpen come first, each name drawn in its own face; fonts installed on the computer follow. ↑/↓ move, Enter picks, Esc closes. Text without a font uses Inter, also on machines without it. A font the computer does not have shows a warning; the engine draws Inter instead. Presets carry no font, so the font survives a preset change.

**Captions**: choose language, accuracy, words per caption (1, 2, 3 or Phrases; 1–3 words also stay within 15 characters), style and font, then Generate. Reel is the default style: Inter 95 px regular, white with a 7.5 px black outline, no box, 2 words. On vertical videos captions keep to the Instagram Reels and TikTok safe area (clear of the top bar, the like and comment rail and the caption and buttons at the bottom; 1080×1920: 250 px top, 180 px right, 500 px bottom, 60 px left): longer captions wrap instead of reaching under the app's interface, and restyling keeps that width. Captions keep a number with the word before it ("iPhone 17") and a sentence does not end on a lone word when the caption before can give one. A caption never runs across a cut. The first run downloads the model with progress. Recognition runs in the background and can be cancelled; while it runs the fields are locked and a starting job shows an indeterminate bar. Captions and Transcript share the language and accuracy and one recognition at a time. The language starts as the computer's language when it is offered (else Detect automatically) and is remembered once picked. Generating again replaces the previous Captions track, keeps the look of the captions there, and the button label says so. With captions on the timeline, picking a style or a font changes every caption at once, as one undo step; words per caption apply when you regenerate.

### Transcript
Text-based editing, as in CapCut. Transcribe timeline recognises each video file whose sound plays on the timeline (music and other audio files are not listened to), once and in the file's own time; a captions run does the same and reuses what is already recognised. The transcript belongs to the file, so it follows every edit (cuts, trims, `Q`/`W`, speed, detached sound, mute, undo and redo), is still there after a restart and is reused when the file is in another project. The refresh button recognises every file again. When a clip's file has no transcript yet, a note says "1 clip not transcribed" with Transcribe; until then Delete and Remove pauses are disabled with that reason. The words flow as text in paragraphs that break where speech pauses for a second or more, each led by its timecode. Pauses longer than the threshold (0.5 s by default) show as chips with their length. While playing, the current word is highlighted and kept in view.

- **Select**: click a word to jump there and select it; drag or `Shift`-click to extend. With the text focused, `←`/`→` move word by word (`Shift` extends), `Home`/`End` jump to the ends, `Enter` jumps to the selection, `Esc` clears it, `Space` plays without leaving the text.
- **Delete**: `Delete`, `Backspace` or the Delete button in the bar under the text, which shows the selection's word count and length. It cuts the selected words and the silence around them, keeping 0.12 s after the word before and 0.08 s before the word after, counted from where those words are heard in the sound, so a cut never clips a syllable, out of every track except audio kept in place, so captions and overlays stay in sync, as one undo step. A selection of pause chips alone removes those pauses. The playhead lands on the cut; a toast with the time removed offers Undo. While the bar shows, toasts sit above it so they never cover Delete. Inside the text these keys never delete timeline clips.
- **Remove pauses**: the button names the count and the time it removes ("Remove 5 pauses · 6.7 s") and cuts every pause chip at once. A pause between words keeps half the threshold next to each word, so a 2 s pause at 0.5 s becomes 0.5 s (where the next clip starts speaking at once, the whole pause stays after the last word of the clip before, so no sliver of a clip is left); silence longer than the threshold before the first word and after the last one is trimmed to 0.08 s and 0.12 s. Pauses and text cuts only lie inside clips that carry speech: clips in which words were recognised. A clip without words, such as B-roll, is never part of a pause and never cut from the text.
- **Keep in place while cutting**: one checkbox per audio track, saved with the project and undoable. Tracks made for music and sound files start checked and keep playing across cuts; sound detached from a video starts unchecked, so it is cut with the picture. The setting also applies to `Q`/`W`.

### Export
`Ctrl+E` or Export opens a dialog: resolution (720p, 1080p, 1440p, 4K, with the resulting pixel size for the canvas), frame rate (24–60, default the project's), quality (High, Recommended, Smaller file) with an estimated file size. Resolution and quality are remembered. Export… asks where to save and adds `.mp4` when the name has no extension. An existing file is only replaced once someone said so: the save dialog asks for the name it returns, and when the final name differs (`.mp4` was added) or the file appears while rendering, the dialog asks “<name> already exists. Replace it?” with Choose another name… and Replace. Tab stays inside the dialog. It can be closed at any time, with `Esc` too unless an export is running (then Keep editing or Cancel export says what happens to it), and focus returns to what opened it; a running export shows its percentage in the top bar and clicking it reopens the dialog. Progress shows percentage and time left, with Cancel. Success offers Show in folder, in the dialog or, when it is closed, in a toast. Failure shows the reason and Retry (same file); with the dialog closed a toast offers Details. Export is disabled with a reason while the timeline is empty.

### Starting
Until the open project is ready the window says "Starting CapOpen…". If that fails, it says why and offers Try again, New project and the recent projects to open instead.

### Errors
An error says in plain words what happened and what to do next; an error toast stays until it is dismissed. The engine's coded errors (`READ_ONLY: …`, `EDIT_REJECTED: …` and the like) are translated where they are shown; an error without a known code is shown as it came.

### Saving
Every edit is saved about a second later. The top bar shows Saving…, Saved, or a failure with an icon and text. An action that changes nothing, such as a failed import, leaves the status as it was.

## Keyboard

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `S` | Split the selected clips under the playhead, else the main-track clip |
| `Q` / `W` | Delete the part of the clip left / right of the playhead |
| `Ctrl+D` | Duplicate selection |
| `Delete`, `Backspace` | Delete selection or the selected transition |
| `Ctrl+Z` / `Ctrl+Shift+Z`, `Ctrl+Y` | Undo / redo |
| `←` `→` (`Shift`) | Step a frame (a second); on a focused clip, go to and select the previous or next clip (`Ctrl`: focus only) |
| `↑` `↓` | On a focused clip, go to and select the clip on the track above or below (`Ctrl`: focus only) |
| `Enter` (`Shift`) | On a focused clip, select it (add or remove it) |
| Menu key, `Shift+F10` | On a focused clip, open its context menu |
| `Alt+←` `Alt+→` (`Shift`) | Nudge the focused clip a frame (a second); on the main track, move it one place |
| `Home` / `End` | Start / end |
| `Ctrl` + wheel, `+` / `-` | Zoom timeline; the playhead stays where it is on screen (moved to the middle if it was out of view), with the wheel the moment under the pointer does |
| `Ctrl+I` | Import |
| `Ctrl+E` | Export |
| `F8` | Focus the newest toast; `Esc` there dismisses it and focus moves to the next toast, then back |
| `Esc` | Cancel a drag or close the open menu or dialog; with none of those, clear the selection |

Shortcuts are ignored while typing in a text field and while the export dialog is open. A focused tab bar keeps `←`/`→` (and `Home`/`End`) to switch tabs, so the playhead does not move. The focused transcript keeps `←`/`→`, `Home`/`End`, `Delete` and `Backspace` for its words.

## Interrupted AI edits

Opening a project with an unfinished AI checkpoint blocks the editor with “An AI edit didn't finish”. “Keep changes” keeps the saved edits. “Restore previous version” restores the checkpoint as one undoable step and offers Undo in a toast. The dialog cannot be dismissed before choosing; a failed recovery keeps it open with the error. While an AI run is open, the top bar shows “AI is editing · <label>” with Stop and edit, and the editing controls are disabled: the inspector, the tools in the left panel, the timeline's edit buttons, track toggles and new transitions, moving and trimming clips, the preview box handles, Ratio and renaming. Disabled buttons stay reachable by keyboard and give that reason; fields are disabled with it as their tooltip. Selecting, playing and scrubbing still work. An edit tried anyway, by shortcut or drop, is not sent and shows “AI is editing. Stop it to edit yourself.” with Stop and edit. That toast stays until the run ends; dismissed, it comes back with the next refused edit. When a run that changed something ends, “AI edit done” offers Undo for the whole run.
