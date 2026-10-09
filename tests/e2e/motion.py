"""Motion presets on a talking clip. The picture is a plain background with a grid and a yellow square, so the
preview and the rendered frames show how far the picture zoomed and where it went. Push in, Pull out and Ken Burns
from the inspector each write two smooth keyframes in one undo step, the strength and the range rewrite the motion,
keyframes changed by hand stay the user's, and an agent gets the same with apply_motion, which leaves a clip that
already moves alone."""
import json, subprocess, time

from e2e.harness import AI_EDITING, CLI, FIXTURES, Bridge, flow, link_models, preview_crop, preview_rect, wait
from e2e.home import resize
from e2e.zooms import GRID, SQUARE, poll

INSPECTOR = 'aside[aria-label=Inspector]'
CLIP = 'return window.__nuzky.store.getState().snap.project.tracks[0].clips[0]'
# The canvas point the zoom keeps in place on a 1080×1920 canvas: the middle of the Reels and TikTok safe area.
FIXED = (480, 835)
FRAME_US = 33_334


def motion_setup(r):
    link_models(r)
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', 'color=c=0x2a2a36:s=1080x1920:r=30',
                    '-i', str(FIXTURES / 'speech.wav'), '-vf', f'{GRID},{SQUARE}', '-shortest', '-c:v', 'libx264',
                    '-preset', 'veryfast', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-b:a', '128k',
                    str(r.work / 'motion-talk.mp4')], check=True)


def click(r, label):
    """Clicks the inspector button with this text, as a person does."""
    return r.s.run(f"""const b = [...document.querySelectorAll('{INSPECTOR} button')].find((b) => b.textContent.trim() === arguments[0]);
        if (!b) return false; b.click(); return true;""", label)


def pressed(r):
    """Text of every pressed button of the Motion section."""
    return r.s.run(f"""const s = [...document.querySelectorAll('{INSPECTOR} section')].find((s) => s.querySelector('h3')?.textContent === 'Motion');
        return s ? [...s.querySelectorAll('button[aria-pressed=true]')].map((b) => b.textContent.trim()) : null;""")


def show_motion(r):
    """Scrolls the inspector to the Motion section; True when every tile label and segment shows whole."""
    whole = r.s.run(f"""const s = [...document.querySelectorAll('{INSPECTOR} section')].find((s) => s.querySelector('h3')?.textContent === 'Motion');
        s.scrollIntoView({{block: 'end'}});
        return [...s.querySelectorAll('button > span.truncate, [role=group] button')].every((e) => e.scrollWidth <= e.clientWidth + 1);""")
    time.sleep(0.5)
    return whole


def keys(r):
    """(time, ease, scale, x, y) of the clip's keyframes, rounded."""
    clip = r.s.run(CLIP, retries=3)
    return [(k['tUs'], k['ease'], round(k['transform']['scale'], 4), round(k['transform']['x'], 4), round(k['transform']['y'], 4))
            for k in clip['keyframes']]


def yellow(image):
    """Width and centre of the yellow square in an image, in its pixels."""
    image = image.convert('RGB')
    width, data = image.width, image.tobytes()
    xs, ys = [], []
    for i in range(image.width * image.height):
        red, green, blue = data[3 * i:3 * i + 3]
        if red > 190 and green > 160 and blue < 90:
            xs.append(i % width)
            ys.append(i // width)
    if len(xs) < 50:
        return None
    return {'width': max(xs) - min(xs) + 1, 'x': (max(xs) + min(xs)) / 2, 'y': (max(ys) + min(ys)) / 2}


def preview_square(r, name, at_us):
    """The square in the preview at `at_us`, with the clip deselected so no selection box covers it."""
    r.s.run('window.__nuzky.store.getState().select([])')
    r.seek(at_us)
    time.sleep(1.5)
    r.shot(name)
    return yellow(preview_crop(r.work / f'{name}.png', preview_rect(r)))


def rendered_square(r, seconds):
    """The square as the export renderer draws the saved project at `seconds`, 1080 px wide."""
    from PIL import Image
    out = r.work / f'frame-{seconds:.3f}.png'
    subprocess.run([str(CLI), 'frame', str(r.saved_project()), f'{seconds:.3f}', str(out), '1080'], env=r.env, check=True,
                   capture_output=True)
    return yellow(Image.open(out))


def saved_keyframes(r):
    return json.loads(r.saved_project().read_text())['tracks'][0]['clips'][0]['keyframes']


@flow('motion', 'Motion presets on a talking clip: Push in from the inspector moves the preview and the rendered frames, '
      'one undo, strength and range rewrite it, hand edits stay, To sentence end follows the transcript, apply_motion '
      'over MCP skips a clip that moves', before=motion_setup)
def motion(r):
    r.import_media(r.work / 'motion-talk.mp4')
    r.add_clip('motion-talk.mp4')
    bridge = Bridge(r, r.saved_project())
    try:
        presets(r, bridge)
    finally:
        bridge.close()


def presets(r, bridge):
    clip = r.s.run(CLIP)
    duration = clip['durationUs']
    r.focus_clip(clip['id'])
    r.check('the Video tab offers Motion with None marked',
            wait(lambda: pressed(r) == ['None', '10%', 'Whole clip'], 10), pressed(r))
    reason = r.s.run(f"""const b = [...document.querySelectorAll('{INSPECTOR} button')].find((b) => b.textContent.trim() === '15%');
        return b.getAttribute('aria-disabled') === 'true' ? b.title : null""")
    r.check('Strength waits for a motion and says so', reason == 'Pick a motion first', reason)
    start = preview_square(r, 'motion-before-start', 0)
    end_before = preview_square(r, 'motion-before-end', duration - FRAME_US)
    r.check('without motion the square keeps its size', start and end_before and start['width'] == end_before['width'],
            [start, end_before])

    # Push in over the whole clip, from the inspector.
    r.focus_clip(clip['id'])
    r.check('Push in is a tile in the Motion section', click(r, 'Push in'))
    pushed = wait(lambda: len(k := keys(r)) == 2 and k, 10)
    fx, fy = FIXED[0] / 1080 - 0.5, FIXED[1] / 1920 - 0.5
    r.check('Push in writes two smooth keyframes from the clip framing to 10 % in, about the safe area centre',
            pushed == [(0, 'smooth', 1.0, 0.0, 0.0), (duration, 'smooth', 1.1, round(-0.1 * fx, 4), round(-0.1 * fy, 4))],
            pushed)
    r.check('the section marks Push in at 10 % over the whole clip', wait(lambda: pressed(r) == ['Push in', '10%', 'Whole clip'], 5),
            pressed(r))
    r.check('the Transform section counts the keyframes', r.s.run(f"return document.querySelector('{INSPECTOR}').innerText.includes('2 keyframes')"))
    show_motion(r)
    r.shot('motion-inspector')

    at_start = preview_square(r, 'motion-start', 0)
    middle = preview_square(r, 'motion-middle', duration // 2)
    end = preview_square(r, 'motion-end', duration - FRAME_US)
    rect = preview_rect(r)
    k = start['width'] / 400 if start else None
    # Where the square's centre moves when the picture grows 10 % about the safe area centre.
    expected = start and (start['x'] + 0.1 * (540 - FIXED[0]) * k, start['y'] + 0.1 * (960 - FIXED[1]) * k)
    r.check('the preview starts on the clip framing, is 5 % in halfway and 10 % in at the end, about the safe area centre',
            start and at_start and middle and end and at_start['width'] == start['width']
            and abs(middle['width'] / start['width'] - 1.05) < 0.02 and abs(end['width'] / start['width'] - 1.1) < 0.02
            and abs(end['x'] - expected[0]) < 1.5 and abs(end['y'] - expected[1]) < 1.5,
            {'start': start, 'middle': middle, 'end': end, 'expected_end_centre': expected, 'preview': rect})
    saved = wait(lambda: len(saved_keyframes(r)) == 2, 10)
    first, last = (rendered_square(r, 0), rendered_square(r, (duration - FRAME_US) / 1e6)) if saved else (None, None)
    r.check('the export renderer draws the same move: the square 10 % larger at the end, its centre where the zoom puts it',
            first and last and abs(last['width'] / first['width'] - 1.1) < 0.01
            and abs(last['x'] - (first['x'] + 6)) < 1.5 and abs(last['y'] - (first['y'] + 12.5)) < 1.5, [first, last])

    r.s.run('document.activeElement?.blur()')
    r.key('z', ctrlKey=True)
    r.check('one Ctrl+Z removes the motion', wait(lambda: keys(r) == [], 10), keys(r))
    undone = preview_square(r, 'motion-undone', duration - FRAME_US)
    r.check('the preview at the end shows the clip framing again', undone and start and undone['width'] == start['width'],
            [undone, start])

    # A strength picked for the clip's motion rewrites it as one more step.
    r.focus_clip(clip['id'])
    click(r, 'Pull out')
    pulled = wait(lambda: (k := keys(r)) and k[0][2] == 1.1 and k, 10)
    r.check('Pull out starts zoomed in and ends on the clip framing', pulled and pulled[1][2:] == (1.0, 0.0, 0.0), pulled)
    click(r, '15%')
    r.check('picking 15 % rewrites it', wait(lambda: (k := keys(r)) and k[0][2] == 1.15, 10), keys(r))
    click(r, '6%')
    r.check('picking 6 % rewrites it', wait(lambda: (k := keys(r)) and k[0][2] == 1.06, 10)
            and wait(lambda: pressed(r) == ['Pull out', '6%', 'Whole clip'], 5), [keys(r), pressed(r)])
    r.s.run('document.activeElement?.blur()')
    r.key('z', ctrlKey=True)
    r.check('Ctrl+Z goes back to 15 %', wait(lambda: (k := keys(r)) and k[0][2] == 1.15, 10), keys(r))

    # Keyframes stay ordinary: Scale at the first one changes it, and the section then marks no preset.
    r.seek(clip['startUs'])
    r.s.run(f"""const i = document.querySelector('{INSPECTOR} input[aria-label="Scale value"]'); i.focus();
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(i, '130');
        i.dispatchEvent(new Event('input', {{bubbles: true}})); i.blur();""")
    edited = wait(lambda: (k := keys(r)) and k[0][2] == 1.3 and k, 10)
    r.check('Scale at a keyframe edits that keyframe', edited and len(edited) == 2 and edited[1][2] == 1.0, keys(r))
    r.check('a hand-edited motion marks no preset', wait(lambda: pressed(r) == ['15%', 'Whole clip'], 5), pressed(r))
    click(r, 'Push in')
    replaced = wait(lambda: (k := keys(r)) and k[0][2] == 1.3 and k[1][2] == 1.495 and k, 10)
    toast = wait(lambda: next((t for t in r.state()['toasts'] if t['text'] == 'Replaced 2 keyframes with Push in'), None), 5)
    r.check('a preset replaces hand-made keyframes from the picture at the playhead, with Undo in a toast', replaced and toast,
            [keys(r), r.state()['toasts']])
    click(r, 'None')
    r.check('None removes the motion', wait(lambda: keys(r) == [], 10), keys(r))

    # From the playhead to the end of the sentence said there, from the transcript.
    job = poll(bridge, bridge.call('transcribe', {'language': 'en', 'model': 'small'}), 300)
    r.check('the speech is recognised', job['status'] == 'done', job)
    words = bridge.call('get_transcript', {})['words']
    first_end = next(i for i, w in enumerate(words) if w['text'].strip().endswith('.'))
    playhead = words[1]['start_us']
    r.seek(playhead)
    click(r, 'Ken Burns')
    wait(lambda: len(keys(r)) == 2, 10)
    click(r, 'Rest of sentence')
    sentence = wait(lambda: len(k := keys(r)) == 2 and k[0][0] > 0 and k, 10)
    r.check('Rest of sentence runs Ken Burns from the playhead to the end of the first sentence',
            sentence and sentence[0][0] == playhead - clip['startUs'] and sentence[1][0] == words[first_end]['end_us'] - clip['startUs']
            and round(sentence[1][2] / sentence[0][2], 3) == 1.15, {'keys': sentence, 'sentence': ' '.join(w['text'] for w in words[:first_end + 1])})
    r.check('the section marks Ken Burns over the sentence', wait(lambda: pressed(r) == ['Ken Burns', '15%', 'Rest of sentence'], 5),
            pressed(r))
    r.check('in a 1024 × 640 window the Motion section shows every label whole',
            resize(r, 1024, 640) and show_motion(r))
    r.shot('motion-sentence-small')
    resize(r, 1440, 900)
    show_motion(r)
    r.shot('motion-sentence')

    # An agent: a clip that already moves is left alone and named; after None, one sentence moves, live and as one undo.
    second = next(i for i, w in enumerate(words) if i > first_end and w['text'].strip().endswith('.'))
    span = [words[first_end + 1]['start_us'], words[second]['end_us']]
    run = bridge.call('begin_run', {'label': 'Push in'})['run_id']
    locked = wait(lambda: r.s.run(f"""const b = [...document.querySelectorAll('{INSPECTOR} button')].find((b) => b.textContent.trim() === 'Push in');
        return b && b.getAttribute('aria-disabled') === 'true' ? b.title : null"""), 10)
    r.check('the Motion tiles are locked with the reason while the agent edits', locked == AI_EDITING, locked)
    skipped = bridge.call('apply_motion', {'run_id': run, 'range_us': span, 'kind': 'pushIn', 'strength': 0.06})
    r.check('apply_motion leaves the clip that moves alone and names it', skipped['skipped'] == [clip['id']]
            and skipped['changed'] == [] and keys(r) == sentence, skipped)
    bridge.call('end_run', {'run_id': run, 'action': 'keep'})
    wait(lambda: not r.state()['aiRun'], 10)
    r.focus_clip(clip['id'])
    click(r, 'None')
    wait(lambda: keys(r) == [], 10)
    run = bridge.call('begin_run', {'label': 'Push in'})['run_id']
    moved = bridge.call('apply_motion', {'run_id': run, 'range_us': span, 'kind': 'pushIn', 'strength': 0.06})
    agent = wait(lambda: len(k := keys(r)) == 2 and k, 10)
    r.check('apply_motion pushes in over the second sentence, shown live',
            moved['changed'] == [clip['id']] and agent and [agent[0][0], agent[1][0]] == [t - clip['startUs'] for t in span]
            and round(agent[1][2] / agent[0][2], 3) == 1.06, [moved, agent])
    bridge.call('end_run', {'run_id': run, 'action': 'keep'})
    r.check('the lock ends with the run', wait(lambda: not r.state()['aiRun'], 10))
    r.s.run('document.activeElement?.blur()')
    r.key('z', ctrlKey=True)
    r.check('one undo removes the agent motion', wait(lambda: keys(r) == [], 10), keys(r))
    r.check('no error toast', not r.errors(), r.errors())
