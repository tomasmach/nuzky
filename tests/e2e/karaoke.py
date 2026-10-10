"""Karaoke captions in the real app: a Czech take captioned with the Karaoke style lights up the word being
said. The preview, the engine's own frame and the exported file must light the same word at the same moment,
nothing while a caption holds after its last word. A word corrected in place keeps lighting up; a caption that
gains a word lights nothing."""
import subprocess, time

from e2e.harness import CLI, FIXTURES, changed_share, flow, link_models, preview_crop, preview_rect, wait

MODEL = 'large-v3-turbo-q5_0'
FPS = 30
HIGHLIGHT = '#ffe14d'

CAPTIONS = """const p = window.__nuzky.store.getState().snap.project;
const track = p.tracks.find((t) => t.kind === 'text' && t.name === 'Captions');
return track ? track.clips.map((c) => ({id: c.id, startUs: c.startUs, durationUs: c.durationUs, text: c.content.text,
  highlight: c.content.style.highlight ?? null, words: c.content.words ?? []})) : [];"""


def karaoke_setup(r):
    link_models(r, ('ggml-silero-v5.1.2.bin', f'ggml-{MODEL}.bin'))


def lit(image):
    """Where the highlight yellow is, as a box in fractions of the picture (left, top, right, bottom), and how many
    pixels have it. Yellow fading into the black outline counts too; white text and the dark take do not."""
    from PIL import Image
    image = (Image.open(image) if not hasattr(image, 'convert') else image).convert('RGB')
    w, h = image.size
    found = [i for i, (r, g, b) in enumerate(image.getdata()) if r > 120 and g > 0.75 * r and b < 0.5 * r]
    if not found:
        return None, 0
    xs, ys = [i % w for i in found], [i // w for i in found]
    return (min(xs) / w, min(ys) / h, (max(xs) + 1) / w, (max(ys) + 1) / h), len(found) / (w * h)


def same_place(a, b, tolerance=0.02):
    return a is not None and b is not None and all(abs(x - y) <= tolerance for x, y in zip(a, b))


def reads_before(a, b):
    """Box a is earlier in reading order than box b: on the line above, or left of it on the same line."""
    return a[3] <= b[1] + 0.005 or (a[1] < b[3] and b[1] < a[3] and a[2] <= b[0] + 0.005)


def frame_at(us):
    """The export frame index whose time lies at `us`, and that frame's exact time."""
    n = round(us * FPS / 1e6)
    return n, (2 * n * 1_000_000 + FPS) // (2 * FPS)


def engine_frame(r, us, name):
    path = r.work / f'{name}.png'
    subprocess.run([str(CLI), 'frame', str(r.saved_project()), f'{us / 1e6:.6f}', str(path), '720'], env=r.env, check=True,
                   capture_output=True)
    return path


def export_frame(r, video, n, name):
    path = r.work / f'{name}.png'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', str(video), '-vf', f'select=eq(n\\,{n})', '-frames:v', '1',
                    '-update', '1', str(path)], check=True)
    r.shots.append(path.name)
    return path


@flow('karaoke', 'Captions in the Karaoke style light up the word being said, in the preview and in the export',
      before=karaoke_setup)
def karaoke(r):
    r.import_media(FIXTURES / 'reel-2.mp4')
    r.add_clip('reel-2.mp4')
    r.s.run(f"window.__nuzky.speech.setState({{model: '{MODEL}', language: 'cs'}})")
    tile = "return [...document.querySelectorAll('button')].find((b) => b.textContent.trim() === 'Karaoke')"

    def captions_tab():
        r.s.run("[...document.querySelectorAll('[role=tab]')].find((t) => t.textContent.trim() === 'Captions')?.click()")
        return r.s.run(tile + ' ? true : false')
    if not r.check('the Captions tab offers the Karaoke style', wait(captions_tab, 10, 0.5)):
        r.shot('captions-tab-missing')
        return
    r.s.run(tile + '.click()')
    r.check('the Karaoke style is picked in the Captions tab', wait(lambda: r.s.run(tile + ".getAttribute('aria-pressed')") == 'true', 5))
    r.shot('captions-tab')
    r.s.run("[...document.querySelectorAll('button')].find((b) => b.textContent.includes('Generate captions')).click()")
    job = wait(lambda: next((j for j in r.state()['jobs'] if j['kind'] == 'captions' and j['status'] != 'running'), None), 900, 1)
    if not r.check('Czech captions are generated', job and job['status'] == 'done', job):
        return
    captions = r.s.run(CAPTIONS)
    r.check('every caption has the Karaoke highlight', captions and all(c['highlight'] == HIGHLIGHT for c in captions),
            [c['highlight'] for c in captions])
    r.check('every caption keeps the time of each of its words',
            all(' '.join(w['text'] for w in c['words']) == c['text'] for c in captions),
            [(c['text'], [w['text'] for w in c['words']]) for c in captions if ' '.join(w['text'] for w in c['words']) != c['text']])
    r.check('the karaoke tile stays marked for the captions on the timeline',
            r.s.run(tile + ".getAttribute('aria-pressed')") == 'true')

    # The first caption of two words each heard for at least four frames: the middle frame of each word.
    def frames(caption):
        out = []
        for word in caption['words'][:2]:
            n, us = frame_at(caption['startUs'] + (word['startUs'] + word['endUs']) / 2)
            out.append((n, us) if caption['startUs'] + word['startUs'] <= us < caption['startUs'] + word['endUs'] else None)
        return out
    caption = next((c for c in captions if len(c['words']) >= 2 and all(w['endUs'] - w['startUs'] >= 4e6 / FPS for w in c['words'][:2])
                    and None not in frames(c)), None)
    if not r.check('a caption of two clearly timed words is there', caption, [c['text'] for c in captions]):
        return
    print(f"  caption \"{caption['text']}\" at {caption['startUs'] / 1e6:.3f} s", flush=True)
    moments = frames(caption)
    words = [w['text'] for w in caption['words'][:2]]

    # The preview, then the engine's own frame of the saved project at the same moment.
    rect = preview_rect(r)
    preview, engine = [], []
    time.sleep(1.5)  # The edit is saved about a second later; `nuzky frame` reads the saved file.
    for (n, us), word, index in zip(moments, words, (1, 2)):
        r.seek(us)
        time.sleep(1.5)
        r.shot(f'preview-word-{index}')
        crop = preview_crop(r.work / f'preview-word-{index}.png', rect)
        preview.append(lit(crop))
        engine.append(lit(engine_frame(r, us, f'engine-word-{index}')))
        r.check(f'the preview lights "{word}" in the middle of it', preview[-1][1] > 0.0005, preview[-1])
        r.check(f'the preview lights "{word}" where the engine does', same_place(preview[-1][0], engine[-1][0]),
                {'preview': preview[-1][0], 'engine': engine[-1][0]})
    r.check(f'"{words[0]}" and "{words[1]}" light up apart, in reading order',
            engine[0][0] and engine[1][0] and reads_before(engine[0][0], engine[1][0]), [engine[0][0], engine[1][0]])
    # A caption stays a moment after its last word before a pause: then nothing is lit.
    holds = [(c, c['startUs'] + c['words'][-1]['endUs'], c['startUs'] + c['durationUs']) for c in captions if c['words']]
    hold = next(((c, (end + stop) // 2) for c, end, stop in holds if stop - end >= 3e6 / FPS), None)
    if r.check('a caption holds after its last word', hold, [(c['text'], stop - end) for c, end, stop in holds]):
        r.seek(hold[1])
        time.sleep(1.5)
        r.shot('preview-hold')
        unlit = [lit(preview_crop(r.work / 'preview-hold.png', rect))[1], lit(engine_frame(r, hold[1], 'engine-hold'))[1]]
        r.check(f'nothing is lit while "{hold[0]["text"]}" holds after its last word', unlit == [0, 0], unlit)

    # The export at 720p shows the same word lit on the same frames.
    target = r.work / 'karaoke.mp4'
    started = r.s.call('window.__nuzky.api.startExport(arguments[0], arguments[1], arguments[2], arguments[3])', str(target),
                       {'resolution': 720, 'fps': FPS, 'quality': 'recommended'}, r.state()['epoch'], False)
    r.check('the export starts', started['ok'], started)
    done = wait(lambda: next((j for j in r.state()['jobs'] if j['kind'] == 'export' and j['status'] != 'running'), None), 300, 1)
    if not r.check('the export finishes', done and done['status'] == 'done', done):
        return
    for (n, us), word, index, expected in zip(moments, words, (1, 2), engine):
        exported = export_frame(r, target, n, f'export-word-{index}')
        box, share = lit(exported)
        r.check(f'export frame {n} lights "{word}" where the engine does', share > 0.0005 and same_place(box, expected[0]),
                {'export': box, 'engine': expected[0], 'share': share})
        changed = changed_share(r.work / f'engine-word-{index}.png', exported)
        r.check(f'export frame {n} matches the rendered one', changed < 0.002, f'{changed:.3%} of pixels differ')

    # The caption's inspector offers the highlight. Fixing a word keeps it on that word; adding one turns it off.
    r.focus_clip(caption['id'])
    toggle = ("return [...document.querySelectorAll('label')].find((l) => l.textContent.trim() === 'Highlight spoken word')"
              "?.querySelector('input')?.checked ?? null")
    r.check('the caption inspector has Highlight spoken word switched on', wait(lambda: r.s.run(toggle) is True, 5))
    show = ("[...document.querySelectorAll('label')].find((l) => l.textContent.trim() === 'Highlight spoken word')"
            ".scrollIntoView({block: 'center'})")
    r.s.run(show)
    r.shot('inspector-caption')
    retext = "window.__nuzky.store.getState().edit({type: 'updateClip', clipId: arguments[0], text: arguments[1]})"
    note = "return document.body.innerText.includes('Words were added or removed, so none lights up')"
    tokens = caption['text'].split(' ')
    corrected = ' '.join([tokens[0] + 'y'] + tokens[1:])
    r.s.call(retext, caption['id'], corrected)
    time.sleep(1.5)  # Saved about a second after the edit.
    fixed = [lit(engine_frame(r, us, f'engine-corrected-{i}'))[0] for i, (_, us) in enumerate(moments, 1)]
    r.check(f'after correcting it to "{tokens[0]}y" that word still lights up when it is said, before the next one',
            fixed[0] and fixed[1] and reads_before(fixed[0], fixed[1]) and not r.s.run(note), fixed)
    r.s.call(retext, caption['id'], corrected + ' navíc')
    r.check('after adding a word the inspector says why no word lights up', wait(lambda: r.s.run(note), 5))
    r.s.run(show)
    r.shot('inspector-edited')
    time.sleep(1.5)
    r.check('the caption with an added word lights no word', lit(engine_frame(r, moments[0][1], 'engine-edited'))[1] == 0)
    r.check('no error toast', not r.errors(), r.errors())


STYLED = """const p = window.__nuzky.store.getState().snap.project;
const track = p.tracks.find((t) => t.kind === 'text' && t.name === 'Captions');
return track ? track.clips.map((c) => ({id: c.id, startUs: c.startUs, durationUs: c.durationUs, text: c.content.text,
  font: c.content.style.fontFamily ?? null, keywords: c.content.style.keywords ?? null, animIn: c.animIn ?? null,
  words: c.content.words ?? []})) : [];"""
KEY_GREEN = '#3ce35a'


def green(image):
    """Where the key word green is, as `lit` finds the highlight yellow: a box in fractions of the picture and its share."""
    from PIL import Image
    image = (Image.open(image) if not hasattr(image, 'convert') else image).convert('RGB')
    w, h = image.size
    found = [i for i, (r, g, b) in enumerate(image.getdata()) if g > 100 and r < 0.6 * g and b < 0.6 * g]
    if not found:
        return None, 0
    xs, ys = [i % w for i in found], [i // w for i in found]
    return (min(xs) / w, min(ys) / h, (max(xs) + 1) / w, (max(ys) + 1) / h), len(found) / (w * h)


def bright(image):
    """How many pixels are near white: a caption's settled fill, which an entry animation dims and shrinks."""
    from PIL import Image
    image = (Image.open(image) if not hasattr(image, 'convert') else image).convert('RGB')
    return sum(1 for r, g, b in image.getdata() if min(r, g, b) > 200)


def tile_js(name):
    return f"return [...document.querySelectorAll('button[aria-pressed]')].find((b) => b.textContent.trim() === {name!r})"


@flow('caption-styles', 'A caption style brings its font, entry animation and key words to every caption; key words show in '
      'the preview and the export, survive regenerating, and My styles outlive a restart', before=karaoke_setup)
def caption_styles(r):
    from e2e.harness import Session, close_window
    from e2e.home import resize
    r.import_media(FIXTURES / 'reel-2.mp4')
    r.add_clip('reel-2.mp4')
    r.s.run(f"window.__nuzky.speech.setState({{model: '{MODEL}', language: 'cs'}})")
    r.s.run("[...document.querySelectorAll('[role=tab]')].find((t) => t.textContent.trim() === 'Captions')?.click()")
    if not r.check('the Captions tab offers the Hormozi green style', wait(lambda: r.s.run(tile_js('Hormozi green') + ' ? true : false'), 10, 0.5)):
        r.shot('captions-tab-missing')
        return
    r.s.run(tile_js('Hormozi green') + '.click()')
    r.check('Hormozi green is picked before there are captions', wait(lambda: r.s.run(tile_js('Hormozi green') + ".getAttribute('aria-pressed')") == 'true', 5))
    r.s.run("[...document.querySelectorAll('button')].find((b) => b.textContent.includes('Generate captions')).click()")
    job = wait(lambda: next((j for j in r.state()['jobs'] if j['kind'] == 'captions' and j['status'] != 'running'), None), 900, 1)
    if not r.check('Czech captions are generated', job and job['status'] == 'done', job):
        return
    captions = r.s.run(STYLED)
    look = {'font': 'Montserrat', 'keywords': {'color': KEY_GREEN, 'pick': 'emphasis'}, 'animIn': {'kind': 'pop', 'durationUs': 160_000}}
    r.check('every caption has the style\'s font, key words and pop-in',
            len(captions) > 2 and all({k: c[k] for k in look} == look for c in captions), [{k: c[k] for k in look} for c in captions[:2]])
    keys = [[w.get('key', False) for w in c['words']] for c in captions]
    print(f"  key words: {[w['text'] for c in captions for w in c['words'] if w.get('key')]}", flush=True)

    # Key words by length instead, for every caption at once.
    r.s.run("[...document.querySelectorAll('[role=group][aria-label=\"Key words\"] button')].find((b) => b.textContent.trim() === 'Longest').click()")
    r.check('Longest marks key words in every caption',
            wait(lambda: all(c['keywords'] == {'color': KEY_GREEN, 'pick': 'longest'} for c in r.s.run(STYLED)), 5))
    r.s.run("document.querySelector('[role=group][aria-label=\"Key words\"]').scrollIntoView({block: 'center'})")
    r.shot('captions-tab')
    if resize(r, 1024, 640):
        r.shot('captions-tab-1024')
        resize(r, 1440, 900)

    # A caption after the first one, of two words or more, in the middle of a word after its pop-in.
    caption = next((c for c in captions[1:] if len(c['words']) >= 2 and c['durationUs'] >= 500_000), None)
    if not r.check('a caption of two words or more is there', caption, [c['text'] for c in captions]):
        return
    first_n, first_us = frame_at(caption['startUs'])
    if first_us < caption['startUs']:
        first_n, first_us = first_n + 1, frame_at(caption['startUs'] + 1e6 / FPS)[1]
    settled_n, settled_us = frame_at(caption['startUs'] + max(300_000, caption['durationUs'] // 2))
    print(f"  caption \"{caption['text']}\" at {caption['startUs'] / 1e6:.3f} s", flush=True)
    rect = preview_rect(r)
    time.sleep(1.5)  # `nuzky frame` reads the saved file, saved about a second after the edit.
    r.seek(settled_us)
    time.sleep(1.5)
    r.shot('preview-key-word')
    preview = green(preview_crop(r.work / 'preview-key-word.png', rect))
    engine = green(engine_frame(r, settled_us, 'engine-key-word'))
    r.check('the preview shows the longest word in the key word green', preview[1] > 0.0003, preview)
    r.check('the preview shows it where the engine does', same_place(preview[0], engine[0]), {'preview': preview[0], 'engine': engine[0]})
    entering = bright(engine_frame(r, first_us, 'engine-first-frame'))
    settled = bright(r.work / 'engine-key-word.png')
    r.check('on the caption\'s first frame its pop-in has only begun', settled > 500 and entering < 0.3 * settled, {'first': entering, 'settled': settled})

    target = r.work / 'caption-styles.mp4'
    started = r.s.call('window.__nuzky.api.startExport(arguments[0], arguments[1], arguments[2], arguments[3])', str(target),
                       {'resolution': 720, 'fps': FPS, 'quality': 'recommended'}, r.state()['epoch'], False)
    r.check('the export starts', started['ok'], started)
    done = wait(lambda: next((j for j in r.state()['jobs'] if j['kind'] == 'export' and j['status'] != 'running'), None), 300, 1)
    if r.check('the export finishes', done and done['status'] == 'done', done):
        exported = green(export_frame(r, target, settled_n, 'export-key-word'))
        r.check(f'export frame {settled_n} shows the key word where the engine does', exported[1] > 0.0003 and same_place(exported[0], engine[0]),
                {'export': exported, 'engine': engine[0]})
        first = bright(export_frame(r, target, first_n, 'export-first-frame'))
        r.check(f'export frame {first_n}, the caption\'s first, shows the pop-in beginning', first < 0.3 * bright(r.work / 'export-key-word.png'),
                {'first': first, 'settled': bright(r.work / 'export-key-word.png')})

    # Save the look as one of My styles.
    r.s.run("document.querySelector('button[aria-label=\"Save the current look as a style\"]').click()")
    field = "document.querySelector('input[aria-label=\"Style name\"]')"
    if not r.check('saving asks for a name in place', wait(lambda: r.s.run(f'return !!{field}'), 5)):
        return
    r.s.run(f"const f = {field}; const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;"
            "set.call(f, 'Green longest'); f.dispatchEvent(new Event('input', {bubbles: true}));"
            "f.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true}));")
    r.check('the style shows under My styles, checked for these captions',
            wait(lambda: r.s.run(tile_js('Green longest') + "?.getAttribute('aria-pressed')") == 'true', 5))
    r.s.run(tile_js('Green longest') + ".scrollIntoView({block: 'center'})")
    r.shot('my-styles')

    # Regenerating keeps the look and scores the same key words again.
    jobs = "return Object.entries(window.__nuzky.store.getState().jobs).map(([id, j]) => ({id, kind: j.kind, status: j.status, message: j.message}))"
    before = {j['id'] for j in r.s.run(jobs)}
    r.s.run("[...document.querySelectorAll('button')].find((b) => b.textContent.includes('Regenerate captions')).click()")
    job = wait(lambda: next((j for j in r.s.run(jobs) if j['kind'] == 'captions' and j['id'] not in before and j['status'] != 'running'), None), 900, 1)
    if not r.check('captions are generated again', job and job['status'] == 'done', job):
        return
    again = r.s.run(STYLED)
    r.check('after regenerating every caption keeps the font, the pop-in and Longest in green',
            again and all(c['font'] == 'Montserrat' and c['animIn'] == look['animIn'] and c['keywords'] == {'color': KEY_GREEN, 'pick': 'longest'} for c in again),
            [{k: c[k] for k in look} for c in again[:2]])
    r.check('regenerating marks the same words as key', [[w.get('key', False) for w in c['words']] for c in again] == keys)
    time.sleep(1.5)
    regenerated = green(engine_frame(r, settled_us, 'engine-regenerated'))
    r.check('the regenerated caption shows its key word in the same place', same_place(regenerated[0], engine[0]),
            {'regenerated': regenerated[0], 'before': engine[0]})

    # Another look, then a restart: My styles is still there and brings the look back.
    r.s.run(tile_js('Reel') + '.click()')
    r.check('Reel replaces the style on every caption', wait(lambda: all(c['keywords'] is None and c['animIn'] is None for c in r.s.run(STYLED)), 5))
    close_window()
    r.check('the window closes', wait(lambda: not _alive(r), 15))
    r.s.close()
    r.s = Session()
    r.check('the app starts again', wait(lambda: r.s.run('return !!window.__nuzky?.store.getState().snap', retries=3), 60))
    r.s.run("window.__nuzky.store.setState({view: 'editor'})")
    r.s.run("[...document.querySelectorAll('[role=tab]')].find((t) => t.textContent.trim() === 'Captions')?.click()")
    if r.check('after a restart My styles still has Green longest', wait(lambda: r.s.run(tile_js('Green longest') + ' ? true : false'), 10, 0.5)):
        r.s.run(tile_js('Green longest') + '.click()')
        r.check('it brings the font, pop-in and key words back to every caption',
                wait(lambda: all(c['font'] == 'Montserrat' and c['animIn'] == look['animIn'] and c['keywords'] == {'color': KEY_GREEN, 'pick': 'longest'}
                                 for c in r.s.run(STYLED)), 5))
    r.check('no error toast', not r.errors(), r.errors())


def _alive(r):
    try:
        return r.s.run('return 1') == 1
    except Exception:
        return False
