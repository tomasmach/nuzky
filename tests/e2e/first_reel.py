"""A new user's first reel, every step from the app's own controls and no agent: a portrait HEVC video from a
phone, Czech captions, retakes, filler words and pauses removed in one go, and the Reels preset. What it checks
is what the viewer gets: the picture upright and filling the frame, captions that say exactly what is still
heard, every kept phrase whole, no word clipped at a cut, and a file Instagram and TikTok take as it is.

The save panel of Export… is native and cannot be driven, so the export starts with the options the dialog
saved once Reels & TikTok was picked."""
import difflib, json, subprocess, time

from e2e.harness import FIXTURES, MODELS, flow, link_models, preview_crop, preview_rect, press, wait
from e2e.layout import resize
from e2e.reel import MODEL, check_captions, check_cut, check_file, check_words, phrases, recognise, truth, words

VIDEO = FIXTURES / 'first-reel.mov'
# The fixture's yellow square in the upright 1080x1920 picture: left, top, right, bottom.
MARKER = (40, 40, 200, 200)
FIND = "[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === arguments[0])"
CHOOSE = """const select = [...document.querySelectorAll('select')].find((s) => [...s.options].some((o) => o.value === arguments[0]));
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(select, arguments[0]);
    select.dispatchEvent(new Event('change', {bubbles: true}));"""


def setup(r):
    missing = [str(p) for p in (VIDEO, MODELS / f'ggml-{MODEL}.bin') if not p.exists()]
    if missing:
        raise RuntimeError(f'run scripts/fixtures.sh first, missing: {missing}')
    link_models(r, ('ggml-silero-v5.1.2.bin', f'ggml-{MODEL}.bin'))


def project(r):
    return r.s.run('return window.__nuzky.store.getState().snap.project', retries=3)


def main_clips(p):
    return next(t for t in p['tracks'] if t['id'] == 'main')['clips']


def captions(p):
    track = next((t for t in p['tracks'] if t['name'] == 'Captions'), None)
    return sorted(track['clips'], key=lambda c: c['startUs']) if track else []


def enabled(r, label):
    return r.s.run(f"const b = {FIND}; return !!b && b.getAttribute('aria-disabled') !== 'true'", label)


def tab(r, name):
    r.s.run('document.querySelector(`[data-tab=${arguments[0]}]`).click()', name)
    time.sleep(0.5)


def marker_box(image):
    """Where the yellow square is in `image` (Pillow), in pixels of the 1080x1920 frame, or None."""
    small = image.convert('RGB').resize((image.width // 4, image.height // 4))
    found = [(x, y) for y in range(small.height) for x in range(small.width)
             if (lambda p: p[0] > 180 and p[1] > 140 and p[2] < 100)(small.getpixel((x, y)))]
    if not found:
        return None
    scale = 1080 / small.width
    xs, ys = [p[0] for p in found], [p[1] for p in found]
    return tuple(round(v * scale) for v in (min(xs), min(ys), max(xs) + 1, max(ys) + 1))


def upright(box, tolerance):
    return box is not None and all(abs(a - b) <= tolerance for a, b in zip(box, MARKER))


@flow('first-reel', 'A new user turns a portrait HEVC phone video into a Reel from the app alone: Czech captions, '
      'retakes and pauses removed in one undo step, the Reels preset; captions match the speech, no word is clipped',
      before=setup, home=True)
def first_reel(r):
    from PIL import Image
    truths = truth([(VIDEO, [p for p in phrases() if p[0] in (1, 2)])])
    r.s.call('window.__nuzky.newProjectFromMedia(arguments[0])', [str(VIDEO)])
    opened = wait(lambda: (p := project(r))['canvas']['width'] == 1080 and p['canvas']['height'] == 1920 and main_clips(p)
                  and r.s.run('return window.__nuzky.store.getState().view') == 'editor', 30)
    r.check('a portrait phone video stored on its side opens as a 9:16 project with the video on the timeline', opened,
            project(r)['canvas'])
    r.seek(500_000)
    time.sleep(1.5)
    r.shot('imported')
    box = marker_box(preview_crop(r.work / 'imported.png', preview_rect(r)))
    r.check('the preview shows the picture upright and filling the frame', upright(box, 40), box)

    # Captions as a new user makes them: Czech, the default accuracy, Generate.
    tab(r, 'captions')
    r.s.run(CHOOSE, 'cs')
    r.s.run(f'{FIND}.click()', 'Generate captions')
    job = wait(lambda: next((j for j in r.state()['jobs'] if j['kind'] == 'captions' and j['status'] != 'running'), None), 600, 1)
    r.check('Czech captions finish', job and job['status'] == 'done', job)
    made = captions(project(r))
    r.check('captions show the Czech words of the video',
            len(made) > 10 and {'mikrofon', 'kamera', 'stativu'} <= set(words(' '.join(c['content']['text'] for c in made))),
            [c['content']['text'] for c in made][:12])
    r.shot('captions')

    # Retakes, filler words and pauses go in one step, from the keyboard.
    tab(r, 'transcript')
    r.check('Remove retakes and pauses is ready once the video is transcribed',
            wait(lambda: enabled(r, 'Remove retakes and pauses'), 30))
    if resize(r, 1024, 640):
        time.sleep(0.5)
        r.shot('remove-retakes-small')
        whole = r.s.run(f'const b = {FIND}; const box = b.getBoundingClientRect();'
                        'return box.right <= innerWidth && box.bottom <= innerHeight && b.scrollWidth <= b.clientWidth', 'Remove retakes and pauses')
        r.check('in the smallest window, 1024 x 640, the button shows whole', whole)
        resize(r, 1440, 900)
    before = project(r)
    r.s.run(f'{FIND}.focus()', 'Remove retakes and pauses')
    r.shot('remove-retakes')
    press('Return')
    r.check('Enter on the focused button cuts the video', wait(lambda: len(main_clips(project(r))) > 1, 15))
    toast = wait(lambda: next((t['text'] for t in r.state()['toasts'] if t['text'].startswith('Removed')), None), 5)
    r.check('a toast says what went: retakes, filler words and pauses',
            toast and 'retake' in toast and 'filler word' in toast and 'pauses over 0.5 s' in toast, toast)
    time.sleep(1)
    r.shot('cut')

    state = project(r)
    cuts = check_cut(r, state, {state['assets'][0]['id']: 1}, truths, 0.6)
    kept = json.loads(r.s.call('window.__nuzky.api.transcriptView(500000).then(JSON.stringify)')['value'])['words']
    shown = words(' '.join(c['content']['text'] for c in captions(state)))
    said = words(' '.join(w['text'] for w in kept))
    r.check('the captions say exactly the words still heard, in order', shown == said,
            {'captions': ' '.join(shown), 'heard': ' '.join(said)})
    showing = lambda t: next((c for c in captions(state) if c['startUs'] <= t < c['startUs'] + c['durationUs']), None)
    off = [(w['text'], round(t / 1e6, 2), c and c['content']['text']) for w in kept
           if (c := showing(t := (w['startUs'] + w['endUs']) // 2)) is None or not set(words(w['text'])) <= set(words(c['content']['text']))]
    r.check('every word is captioned while it is heard', not off, off)

    # One undo takes the whole cleanup back, captions included; redo brings it again.
    r.s.run('document.activeElement?.blur()')
    r.key('z', ctrlKey=True)
    r.check('one undo brings back the video and the captions as they were',
            state['tracks'] != before['tracks'] and wait(lambda: project(r)['tracks'] == before['tracks'], 10))
    r.key('z', ctrlKey=True, shiftKey=True)
    r.check('redo cuts them again', wait(lambda: project(r)['tracks'] == state['tracks'], 10))

    # The Reels preset as the user picks it in the export dialog.
    r.key('e', ctrlKey=True)
    time.sleep(0.8)
    r.s.run("[...document.querySelectorAll('[role=dialog] button')].find((b) => b.textContent.trim().startsWith('Reels')).click()")
    time.sleep(0.5)
    r.shot('export-dialog')
    dialog = r.s.run("return document.querySelector('[role=dialog]')?.textContent ?? ''")
    r.check('the export dialog offers Reels & TikTok at 1080x1920 with Reels loudness, ready to export',
            enabled(r, 'Export…') and '1080×1920' in dialog and 'Loudness −14 LUFS' in dialog, dialog[:300])
    options = json.loads(r.s.run("return localStorage.getItem('nuzky.export')"))
    r.key('Escape')
    out = r.work / 'first-reel.mp4'
    started = time.time()
    begun = r.s.call('window.__nuzky.api.startExport(arguments[0], arguments[1], arguments[2], false)', str(out),
                     {**options, 'fps': 30}, r.state()['epoch'])
    r.check('the export starts with the Reels preset', begun['ok'] and options['preset'] == 'reels', [begun, options])
    job = wait(lambda: (j := r.s.run('return window.__nuzky.store.getState().jobs[arguments[0]] ?? null', begun.get('value')))
               and j['status'] != 'running' and j, 600, 1)
    r.check('the export finishes', job and job['status'] == 'done', job)
    print(f'  export took {time.time() - started:.0f} s', flush=True)

    check_file(r, out, max(c['startUs'] + c['durationUs'] for t in state['tracks'] for c in t['clips']) / 1e6, cuts)
    frame = r.work / 'export-start.png'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-ss', '0.5', '-i', str(out), '-frames:v', '1', '-update', '1', str(frame)], check=True)
    r.shots.append(frame.name)
    box = marker_box(Image.open(frame))
    r.check('the export shows the picture upright and filling the frame', upright(box, 12), box)
    check_captions(r, out, captions(state))
    check_words(r, out, {'words': kept}, truths)
    check_whole_words(r, out, kept, cuts)
    r.check('no error toast', not r.errors(), r.errors())


def check_whole_words(r, out, kept, cuts):
    """The export heard again: every word next to a cut is recognised as in the video it came from, so no cut
    clipped it. Compared with the app's transcript of what it kept, which the same model recognised."""
    exported = recognise(out, r.work)
    expected = [''.join(words(w['text'])) for w in kept]
    heard = [''.join(words(w['text'])) for w in exported]
    # Recognition of the louder export spells a word a little differently now and then ("nejdžív", "Nejdřív")
    # or splits it ("TikTok", "Tik Tok"); a clipped word is missing or no longer close.
    matched = set()
    for op, i1, i2, j1, j2 in difflib.SequenceMatcher(None, expected, heard, autojunk=False).get_opcodes():
        near = heard[max(0, j1 - 1):j2 + 1]
        near += [a + b for a, b in zip(near, near[1:])]
        matched.update(i for i in range(i1, i2) if op == 'equal' or max(
            (difflib.SequenceMatcher(None, expected[i], n).ratio() for n in near), default=0) >= 0.75)
    edges = set()
    for at, *_ in cuts:
        before = [i for i, w in enumerate(kept) if w['endUs'] / 1e6 <= at + 0.05]
        after = [i for i, w in enumerate(kept) if w['startUs'] / 1e6 >= at - 0.05]
        edges.update(before[-1:] + after[:1])
    clipped = [(kept[i]['text'], round(kept[i]['startUs'] / 1e6, 2)) for i in sorted(edges) if i not in matched]
    r.check(f'every word next to one of the {len(cuts)} cuts is heard whole in the export', edges and not clipped,
            {'not recognised again': clipped, 'heard': ' '.join(w['text'] for w in exported)})
