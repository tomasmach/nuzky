import json, statistics, subprocess, time

from e2e.ducking import RATE, TONE, WINDOW, decode, tone_levels
from e2e.harness import FIXTURES, Bridge, export, flow, preview_brightness, wait, webdriver

# Presses a fade dot of a clip and drags it `arguments[2]` px along the timeline, without letting go.
FADE_DRAG = """const knob = document.querySelector(`[data-clip-id='${arguments[0]}'] [data-fade='${arguments[1]}']`);
if (!knob) return false;
const b = knob.getBoundingClientRect();
const at = (type, dx) => new PointerEvent(type, {bubbles: true, button: 0, pointerId: 1,
  clientX: b.left + b.width / 2 + dx, clientY: b.top + b.height / 2});
knob.dispatchEvent(at('pointerdown', 0));
window.dispatchEvent(at('pointermove', arguments[2]));
return true;"""
RELEASE = "window.dispatchEvent(new PointerEvent('pointerup', {bubbles: true, button: 0, pointerId: 1}))"
TIP = "return [...document.querySelectorAll('section[aria-label=Timeline] div')].some((d) => d.textContent === arguments[0])"
FADES = """const c = window.__nuzky.store.getState().snap.project.tracks.flatMap((t) => t.clips).find((c) => c.id === arguments[0]);
return [c.content.fadeInUs, c.content.fadeOutUs];"""
KNOBS = "return document.querySelectorAll(`[data-clip-id='${arguments[0]}'] [data-fade]`).length"
SELECTION = 'return window.__nuzky.store.getState().selection'


@flow('edit', 'Import two clips, split one with S, delete a piece with Delete, undo, find the result saved, see imported music '
      'in the Audio tab, and fade a sound in by dragging the dot on its clip, heard in the export; Esc cancels such a drag')
def edit(r):
    r.import_media(FIXTURES / 'talk.mp4', FIXTURES / 'wide.mp4')
    for name in ('talk.mp4', 'wide.mp4'):
        r.add_clip(name)
    clips = r.track()
    r.check('clips sit back to back on the main track', clips[1]['startUs'] == clips[0]['startUs'] + clips[0]['durationUs'], clips)
    r.check('both clips get thumbnails', wait(lambda: len(r.state()['thumbs']) == 2), r.state()['thumbs'])
    r.seek(2_000_000)
    r.focus_clip(clips[0]['id'])
    r.key('s')
    split = wait(lambda: (c := r.track()) and len(c) == 3 and c)
    r.check('S splits the clip at the playhead', split and split[0]['durationUs'] == 2_000_000, split)
    piece = split[1]
    r.focus_clip(piece['id'])
    r.key('Delete')
    deleted = wait(lambda: (c := r.track()) and len(c) == 2 and c)
    end = lambda clips: clips[-1]['startUs'] + clips[-1]['durationUs']
    r.check('Delete removes the piece and closes the gap', deleted and end(deleted) == end(split) - piece['durationUs'], deleted)
    r.key('z', ctrlKey=True)
    undone = wait(lambda: (c := r.track()) and len(c) == 3 and c)
    r.check('Ctrl+Z brings the piece back', undone == split, undone)
    time.sleep(1.5)
    r.shot('timeline')
    # talk.mp4 is portrait and fills the 9:16 canvas; wide.mp4, which followed the deleted piece, leaves it black at the top.
    top = preview_brightness(r, r.work / 'timeline.png', 0.1)
    r.check('the preview shows the restored piece under the playhead', top > 40, round(top, 1))
    saved = lambda: len(json.loads(r.saved_project().read_text())['tracks'][0]['clips'])
    r.check('the project file on disk has the same three clips', wait(lambda: saved() == 3, 10), saved())
    # Opening the Audio tab once blanked the whole window: its store selector built a new list on every read.
    r.import_media(FIXTURES / 'music.mp3')
    r.s.run("[...document.querySelectorAll('[role=tab]')].find((t) => t.textContent.trim() === 'Audio').click()")
    listed = wait(lambda: r.s.run("return document.body.innerText.includes('music.mp3')"), 5)
    r.check('the Audio tab opens and lists the imported music', listed)

    # Fades as in CapCut: a 4 s tone on an audio track from 0 s, its fade-in dot dragged 100 px at 100 px a second.
    tone = r.work / 'tone.wav'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', f'sine=frequency={TONE}:sample_rate={RATE}:duration=4',
                    '-c:a', 'pcm_s16le', str(tone)], check=True)
    r.import_media(tone)
    asset = next(a['id'] for a in r.state()['assets'] if a['name'] == 'tone.wav')
    r.s.call("window.__nuzky.store.getState().edit({type: 'addClip', assetId: arguments[0], startUs: 0, trackId: null})", asset)
    clip = wait(lambda: next((c['id'] for t in r.state()['tracks'] for c in t['clips'] if c['assetId'] == asset), None), 10)
    if not clip:
        raise RuntimeError('the tone was not placed')
    r.seek(0)
    r.s.run('window.__nuzky.store.getState().setZoom(100)')
    r.focus_clip(clip)
    time.sleep(0.5)
    r.check('the selected sound clip shows a fade dot at each end', r.s.run(KNOBS, clip) == 2, r.s.run(KNOBS, clip))
    r.s.run(FADE_DRAG, clip, 'in', 100)
    r.check('dragging the fade-in dot 100 px shows "Fade in 1.0 s"', wait(lambda: r.s.run(TIP, 'Fade in 1.0 s'), 5))
    r.shot('fade-drag')
    r.s.run(RELEASE)
    r.check('letting go fades the sound in over 1 s', wait(lambda: r.s.run(FADES, clip) == [1_000_000, 0], 5), r.s.run(FADES, clip))
    project = lambda: json.loads(r.saved_project().read_text())
    saved_fades = lambda: next(([c['content']['fadeInUs'], c['content']['fadeOutUs']] for t in project()['tracks']
                                for c in t['clips'] if c['id'] == clip), None)
    r.check('the project file on disk has the 1 s fade-in', wait(lambda: saved_fades() == [1_000_000, 0], 10), saved_fades())

    # Esc in the middle of a drag puts everything back; the next Esc clears the selection.
    r.s.run(FADE_DRAG, clip, 'out', -80)
    r.check('dragging the fade-out dot 80 px shows "Fade out 0.8 s"', wait(lambda: r.s.run(TIP, 'Fade out 0.8 s'), 5))
    r.key('Escape')
    r.s.run(RELEASE)
    time.sleep(1.5)
    r.check('Esc in the middle of the drag changes nothing, on screen or on disk',
            r.s.run(FADES, clip) == [1_000_000, 0] and saved_fades() == [1_000_000, 0], [r.s.run(FADES, clip), saved_fades()])
    r.check('the value hides and the clip stays selected', not r.s.run(TIP, 'Fade out 0.8 s') and r.s.run(SELECTION) == [clip],
            r.s.run(SELECTION))
    r.key('Escape')
    r.check('the next Esc clears the selection', wait(lambda: r.s.run(SELECTION) == [], 5), r.s.run(SELECTION))
    r.focus_clip(clip)
    webdriver('POST', r.s.path + '/window/rect', {'width': 1024, 'height': 640})
    wait(lambda: r.s.run('return [innerWidth, innerHeight]') == [1024, 640], 5)
    time.sleep(1)
    r.shot('fade-small-window')

    # What a listener gets: the tone's level in 50 ms windows against its level once the fade is over (1.5-3.5 s).
    # The fade is linear, so a window starting at t s is about 20·log10(t + 0.025) dB down.
    levels = tone_levels(decode(export(r, 'fade.mp4'), f'highpass=f={TONE - 500},highpass=f={TONE - 500}'))
    full = statistics.median(levels[30:70])
    at = lambda seconds: levels[round(seconds * RATE / WINDOW)] - full
    rise = ', '.join(f'{s:.2f} s {at(s):+.1f} dB' for s in (0, 0.25, 0.5, 0.75, 1.05))
    r.check('the export starts the tone at least 20 dB down', at(0) < -20, rise)
    r.check('half way through the fade it is 6 dB down', abs(at(0.5) + 5.6) < 1.5, rise)
    r.check('after the fade it plays at full level', abs(at(1.05)) < 1, rise)

    # While an AI agent edits, the dots are gone, as the trim handles are.
    bridge = Bridge(r, r.saved_project())
    try:
        run = bridge.call('begin_run', {'label': 'Look at the fades'})
        r.check('while the AI edits, the clip has no fade dots',
                wait(lambda: r.state()['aiRun'], 10) and wait(lambda: r.s.run(KNOBS, clip) == 0, 5), r.s.run(KNOBS, clip))
        bridge.call('end_run', {'run_id': run['run_id'], 'action': 'keep'})
    finally:
        bridge.close()
    r.check('they come back when the run ends', wait(lambda: r.s.run(KNOBS, clip) == 2, 10), r.s.run(KNOBS, clip))

    r.check('no error toast', not r.errors(), r.errors())
