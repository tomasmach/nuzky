"""Zooms on emphasis in three Czech takes. The Transcript tab suggests punch-ins on the sentences said with
emphasis, Apply zooms in on them as one undo step, and the preview shows the picture scaled up there; an agent gets
the same over MCP. The takes keep their sound and get a grid and a yellow square in the middle of the picture, so
the preview shows how much the picture is scaled."""
import json, subprocess, time

from e2e.harness import (AI_EDITING, FIXTURES, Bridge, flow, link_models, preview_crop, preview_rect, preview_redraw,
                         wait, webdriver)

MODEL = 'large-v3-turbo-q5_0'
# The yellow square: 400 px wide in the 1080 px frame, centred.
SQUARE = 'drawbox=x=340:y=760:w=400:h=400:color=0xffd400@1:t=fill'
GRID = 'drawgrid=w=135:h=160:t=6:color=white@0.8'
PROJECT = 'return window.__capopen.store.getState().snap.project'


def takes(r):
    return [r.work / f'zoom-take-{n}.mp4' for n in (1, 2, 3)]


def zooms_setup(r):
    link_models(r, ('ggml-silero-v5.1.2.bin', f'ggml-{MODEL}.bin'))
    for n, out in enumerate(takes(r), 1):
        subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', str(FIXTURES / f'reel-{n}.mp4'), '-vf', f'{GRID},{SQUARE}',
                        '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', '-c:a', 'copy', str(out)], check=True)


def poll(bridge, job, timeout):
    end = time.time() + timeout
    while True:
        state = bridge.call('job', {'job_id': job['job_id'], 'action': 'get'})
        if state['status'] != 'running' or time.time() > end:
            return state
        time.sleep(1)


def press(r, key):
    """A real key press, as WebDriver's keyboard sends it to the focused element, so a focused button acts on Enter."""
    keys = [{'type': 'keyDown', 'value': key}, {'type': 'keyUp', 'value': key}]
    webdriver('POST', r.s.path + '/actions', {'actions': [{'type': 'key', 'id': 'keyboard', 'actions': keys}]})
    webdriver('DELETE', r.s.path + '/actions')


ENTER = '\ue007'  # WebDriver's Enter key


def main(r):
    return r.s.run(PROJECT, retries=3)['tracks'][0]['clips']


def pieces(clips):
    """(start, end, scale, asset, source in) of each main-track clip."""
    return [(c['startUs'], c['startUs'] + c['durationUs'], round(c['content']['transform']['scale'], 4), c['content']['assetId'],
             c['content']['sourceInUs']) for c in clips]


def merged(clips):
    """The main track with split pieces that play on from each other joined again, scale left out."""
    out = []
    for start, end, _, asset, source in pieces(clips):
        last = out[-1] if out else None
        if last and last[3] == asset and last[1] == start and last[4] + (last[1] - last[0]) == source:
            out[-1] = (last[0], end, None, asset, last[4])
        else:
            out.append((start, end, None, asset, source))
    return out


def square(r, name):
    """Width, height and centre of the yellow square in the preview, in screen pixels."""
    crop = preview_crop(r.work / f'{name}.png', preview_rect(r)).convert('RGB')
    width, data = crop.width, crop.tobytes()
    xs, ys = [], []
    for i in range(crop.width * crop.height):
        red, green, blue = data[3 * i:3 * i + 3]
        if red > 190 and green > 160 and blue < 90:
            xs.append(i % width)
            ys.append(i // width)
    if len(xs) < 50:
        return None
    return {'width': max(xs) - min(xs) + 1, 'height': max(ys) - min(ys) + 1, 'x': (max(xs) + min(xs)) / 2, 'y': (max(ys) + min(ys)) / 2}


@flow('zooms', 'Suggested zooms on three Czech takes: marked sentences, one-step Apply that scales the picture up there '
      'and nothing else, Ctrl+Z, and the same suggestions and apply over MCP', before=zooms_setup)
def zooms(r):
    r.import_media(*takes(r))
    for take in takes(r):
        r.add_clip(take.name)
    r.check('the three takes sit on the main track', len(r.track()) == 3, r.track())
    bridge = Bridge(r, r.saved_project())
    try:
        suggest_and_apply(r, bridge)
    finally:
        bridge.close()


def suggest_and_apply(r, bridge):
    started = time.time()
    job = poll(bridge, bridge.call('transcribe', {'language': 'cs', 'model': MODEL}), 600)
    r.check('Czech speech in all three takes is recognised', job['status'] == 'done', job)
    print(f'  recognition took {time.time() - started:.0f} s', flush=True)
    transcript = bridge.call('get_transcript', {})
    words = transcript['words']

    r.s.run("[...document.querySelectorAll('[role=tab]')].find((t) => t.textContent.trim() === 'Transcript').click()")
    r.check('the Transcript tab offers Suggest zooms', wait(lambda: r.s.run(
        "const b = document.querySelector('[data-suggest-zooms]'); return !!b && b.getAttribute('aria-disabled') !== 'true'"), 30))
    # From the keyboard: Enter on the focused button suggests, and Apply takes focus.
    r.s.run("document.querySelector('[data-suggest-zooms]').focus()")
    press(r, ENTER)
    bar = wait(lambda: r.s.run("return document.querySelector('[data-zoom-bar]')?.textContent ?? null"), 15)
    r.check('Suggest zooms opens the bar with Dismiss and Apply', bar and 'Dismiss' in bar and 'Apply' in bar, bar)
    marked = r.s.run("""const words = {};
        for (const w of document.querySelectorAll('[data-zoom]')) (words[w.dataset.zoom] ??= []).push(w.textContent.trim());
        return Object.values(words).map((w) => w.join(' '));""")
    analysis = bridge.call('analyze', {'kind': 'emphasis'})
    suggested = [' '.join(w['text'].strip() for w in words[z['from']:z['to'] + 1]) for z in analysis['zooms']]
    r.check('the marked sentences are the ones analyze(emphasis) suggests', marked and marked == suggested,
            {'marked': marked, 'analysis': analysis['zooms']})
    before = main(r)
    duration = sum(c['durationUs'] for c in before)
    r.check('the suggestions are subtle: about one per 12 s, 5 s apart, 1.15-1.3x',
            len(analysis['zooms']) <= max(1, (duration + 6_000_000) // 12_000_000)
            and all(b['start_us'] - a['end_us'] >= 5e6 for a, b in zip(analysis['zooms'], analysis['zooms'][1:]))
            and all(1.15 <= z['scale'] <= 1.3 for z in analysis['zooms']), {'duration_us': duration, 'zooms': analysis['zooms']})
    focused = r.s.run("return document.activeElement?.hasAttribute('data-zoom-apply') ?? false")
    r.check('Apply has keyboard focus', focused)
    r.shot('zoom-suggestions')

    project_before = r.s.run(PROJECT)
    press(r, ENTER)
    applied = wait(lambda: (c := main(r)) and len(c) > len(before) and c, 15)
    r.check('Apply splits the main track', applied, applied and pieces(applied))
    toast = wait(lambda: next((t for t in r.state()['toasts'] if t['text'].startswith('Zoomed in on')), None), 5)
    r.check('a toast names the zooms and offers Undo', toast and str(len(analysis['zooms'])) in toast['text'], r.state()['toasts'])
    r.check('the suggestions close after Apply', not r.s.run("return !!document.querySelector('[data-zoom-bar]')"))
    zoomed = [p for p in pieces(applied) if p[2] != 1.0]
    expected = []
    for z in analysis['zooms']:
        first, last = words[z['from']]['start_us'], words[z['to']]['end_us']
        # Midway into the silence, at most 0.15 s out; a cut closer than 0.3 s takes the edge.
        match = [p for p in zoomed if first - 450_000 <= p[0] <= first and last <= p[1] <= last + 450_000 and abs(p[2] - z['scale']) < 1e-3]
        expected.append({'zoom': z['text'], 'pieces': match})
    r.check('each suggested sentence plays zoomed by its factor, just around its words',
            all(len(e['pieces']) == 1 for e in expected) and len(zoomed) == len(expected), {'expected': expected, 'zoomed': zoomed})
    project_after = r.s.run(PROJECT)
    same = (project_after['assets'] == project_before['assets'] and project_after['tracks'][1:] == project_before['tracks'][1:]
            and project_after['canvas'] == project_before['canvas'] and merged(applied) == merged(before))
    r.check('nothing else changed: the pieces play the same source back to back', same,
            {'before': merged(before), 'after': merged(applied)})

    # The picture: the yellow square is as much bigger as the zoom, around the same centre.
    first = next(p for p in pieces(applied) if p[2] != 1.0)
    at = (first[0] + first[1]) // 2
    r.s.run('window.__capopen.store.getState().select([])')
    r.seek(at)
    time.sleep(1.5)
    r.shot('zoom-applied')
    zoomed_square = square(r, 'zoom-applied')
    r.s.run('document.activeElement?.blur()')
    r.key('z', ctrlKey=True)
    restored = wait(lambda: (c := main(r)) and len(c) == len(before) and c, 10)
    r.check('Ctrl+Z restores the takes as they were', restored and pieces(restored) == pieces(before), restored and pieces(restored))
    _, redrawn = preview_redraw(r, 'zoom-undone', preview_crop(r.work / 'zoom-applied.png', preview_rect(r)))
    plain_square = square(r, 'zoom-undone')
    ratio = zoomed_square and plain_square and zoomed_square['width'] / plain_square['width']
    r.check('the preview shows the picture scaled up by the zoom at that moment',
            redrawn and ratio and abs(ratio - first[2]) < 0.05 and abs(zoomed_square['height'] / plain_square['height'] - first[2]) < 0.05
            and abs(zoomed_square['x'] - plain_square['x']) < 3 and abs(zoomed_square['y'] - plain_square['y']) < 3,
            {'zoomed': zoomed_square, 'plain': plain_square, 'ratio': ratio and round(ratio, 3), 'scale': first[2]})

    # An agent: the same analysis twice, applied in a run, the panel locked meanwhile, one undo.
    again = bridge.call('analyze', {'kind': 'emphasis'})
    r.check('analyze(emphasis) gives the same result twice', again == bridge.call('analyze', {'kind': 'emphasis'})
            and again['zooms'] == analysis['zooms'], again)
    run = bridge.call('begin_run', {'label': 'Punch in'})['run_id']
    locked = wait(lambda: r.s.run("const b = document.querySelector('[data-suggest-zooms]');"
                                  "return b && b.getAttribute('aria-disabled') === 'true' ? b.title : null"), 10)
    r.check('Suggest zooms is locked with the reason while the agent edits', locked == AI_EDITING, locked)
    zooms = [{'from': z['from'], 'to': z['to'], 'scale': z['scale']} for z in again['zooms']]
    result = bridge.call('apply_zooms', {'run_id': run, 'request_id': 'punch-in', 'transcript_key': again['transcript_key'],
                                         'zooms': zooms})
    r.check('apply_zooms zooms every sentence and skips no clip', len(result['ranges']) == len(zooms) and result['skipped'] == [],
            result)
    r.check('the agent zooms show live', wait(lambda: pieces(main(r)) == pieces(applied), 10), pieces(main(r)))
    bridge.call('end_run', {'run_id': run, 'action': 'keep'})
    r.check('the lock ends with the run', wait(lambda: not r.state()['aiRun'], 10))
    r.shot('agent-zooms')
    r.s.run('document.activeElement?.blur()')
    r.key('z', ctrlKey=True)
    r.check('one undo removes the whole run', wait(lambda: pieces(main(r)) == pieces(before), 10), pieces(main(r)))
    saved = lambda: json.loads(r.saved_project().read_text())['tracks'][0]['clips']
    r.check('the project on disk is back to the three takes', wait(lambda: len(saved()) == 3, 10), len(saved()))
    r.check('no error toast', not r.errors(), r.errors())
