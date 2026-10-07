"""Correcting what Whisper misheard in a Czech take: the user types the word in the transcript and the caption
changes with it; generating the captions again, reopening the app and undo all keep the word and its caption
together. Then an agent does the same over MCP."""
import difflib, json, time

from e2e.harness import (FIXTURES, MODELS, Bridge, Session, flow, link_models, preview_crop, preview_rect,
                         preview_redraw, wait, webdriver)
from e2e.reel import phrases, words as plain

TAKE = FIXTURES / 'reel-2.mp4'
MODEL = 'large-v3-turbo-q5_0'
ELEMENT = 'element-6066-11e4-a52e-4f735466cecf'
ENTER = '\ue007'  # WebDriver's Enter key
# Words of the transcript as the panel shows them; pause chips carry an aria-label instead of text.
SHOWN = """return [...document.querySelectorAll('[role=listbox][aria-label=Transcript] [role=option][data-t]')]
    .filter((e) => !e.hasAttribute('aria-label'))
    .map((e) => ({t: Number(e.dataset.t), text: e.textContent, title: e.title || null,
                  underline: getComputedStyle(e).textDecorationStyle}));"""


def setup(r):
    missing = [str(p) for p in (TAKE, MODELS / f'ggml-{MODEL}.bin') if not p.exists()]
    if missing:
        raise RuntimeError(f'run scripts/fixtures.sh first, missing: {missing}')
    link_models(r, ('ggml-silero-v5.1.2.bin', f'ggml-{MODEL}.bin'))


def misheard(recognised):
    """Words recognition got wrong, found by comparing with what the take says (diacritics aside, which espeak
    and Whisper blur): [(index in recognised, recognised word, the word as said with the recognised punctuation)]."""
    said = ' '.join(text for take, _, _, text in phrases() if take == 2).split()
    a = [' '.join(plain(w)) for w in recognised]
    b = [' '.join(plain(w)) for w in said]
    found = []
    for op, i1, i2, j1, j2 in difflib.SequenceMatcher(None, a, b, autojunk=False).get_opcodes():
        if op == 'replace' and i2 - i1 == j2 - j1:
            for k in range(i2 - i1):
                word, right = recognised[i1 + k], said[j1 + k]
                trail = word[len(word.rstrip('.,!?;:…')):]
                found.append((i1 + k, word, right.strip('.,!?;:…') + trail))
    # "oka" for "okna" is the mistake the user reported; ASCII words type the same on every keyboard layout.
    return sorted(found, key=lambda f: (plain(f[1]) != ['oka'], not f[2].isascii()))


def caption_texts(r):
    track = next((t for t in r.state()['tracks'] if t['name'] == 'Captions'), None)
    return [c['text'] for c in track['clips']] if track else []


def caption_with(r, word):
    """The caption clip that shows `word`, compared as the engine does: without case or punctuation."""
    track = next((t for t in r.state()['tracks'] if t['name'] == 'Captions'), {'clips': []})
    wanted = plain(word)
    return next((c for c in track['clips'] if any(plain(c['text'])[n:n + len(wanted)] == wanted
                                                  for n in range(len(plain(c['text']))))), None)


def saved(r):
    project = json.loads(r.saved_project().read_text())
    texts = [c['content']['text'] for t in project['tracks'] if t['name'] == 'Captions' for c in t['clips']]
    return [{k: c[k] for k in ('original', 'text')} for c in project.get('wordCorrections', [])], texts


def tab(r, name):
    r.s.run('document.querySelector(`[data-tab=${arguments[0]}]`).click()', name)
    time.sleep(0.5)


def shown(r):
    return r.s.run(SHOWN, retries=3) or []


def actions(r, *devices):
    webdriver('POST', r.s.path + '/actions', {'actions': list(devices)})
    webdriver('DELETE', r.s.path + '/actions')


def double_click(r, t):
    """Two real clicks on the word, so the webview itself makes the double-click."""
    element = webdriver('POST', r.s.path + '/element', {'using': 'css selector', 'value': f'[data-t="{t}"]'})
    click = [{'type': 'pointerDown', 'button': 0}, {'type': 'pointerUp', 'button': 0}]
    actions(r, {'type': 'pointer', 'id': 'mouse', 'parameters': {'pointerType': 'mouse'},
                'actions': [{'type': 'pointerMove', 'origin': {ELEMENT: element[ELEMENT]}, 'x': 0, 'y': 0}] + click + click})


def type_keys(r, text):
    keys = [a for c in text for a in ({'type': 'keyDown', 'value': c}, {'type': 'keyUp', 'value': c})]
    actions(r, {'type': 'key', 'id': 'keyboard', 'actions': keys})


def field(r):
    return r.s.run("const f = document.querySelector('[role=listbox][aria-label=Transcript] input');"
                   "return f && {value: f.value, focused: document.activeElement === f};")


def generate_captions(r):
    tab(r, 'captions')
    jobs = lambda: [j for j in r.state()['jobs'] if j['kind'] == 'captions']
    count = len(jobs())
    r.s.run("[...document.querySelectorAll('button')].find((b) => /enerate captions/.test(b.textContent)).click()")
    return wait(lambda: next((j for j in jobs()[count:] if j['status'] != 'running'), None), 600, 1)


def close_window():
    """Closes the app window as its title-bar button does."""
    from Xlib import X, display
    from Xlib.protocol import event
    d = display.Display()
    protocols, delete = d.intern_atom('WM_PROTOCOLS'), d.intern_atom('WM_DELETE_WINDOW')
    stack = [d.screen().root]
    while stack:
        w = stack.pop()
        try:
            name = w.get_wm_name()
            stack.extend(w.query_tree().children)
        except Exception:
            continue
        if name in ('CapOpen', b'CapOpen'):
            w.send_event(event.ClientMessage(window=w, client_type=protocols, data=(32, [delete, X.CurrentTime, 0, 0, 0])))
    d.sync()


def alive(r):
    try:
        r.s.run('return 1')
        return True
    except Exception:
        return False


@flow('corrections', 'A misheard word typed right in the transcript changes its caption, and regenerating captions, '
      'reopening the app and undo keep word and caption together; an agent does the same over MCP', before=setup)
def corrections(r):
    r.import_media(TAKE)
    r.add_clip('reel-2.mp4')
    r.s.run("window.__capopen.speech.setState({model: arguments[0], language: 'cs'})", MODEL)
    job = generate_captions(r)
    r.check('Czech captions are generated', job and job['status'] == 'done' and caption_texts(r), job)
    tab(r, 'transcript')
    r.check('the transcript shows the words', wait(lambda: len(shown(r)) > 10, 30), shown(r))
    words = shown(r)
    wrong = misheard([w['text'] for w in words])
    if not r.check('recognition misheard a word to correct', wrong, [w['text'] for w in words]):
        return
    (index, word, right), *rest = wrong
    target = words[index]['t']
    caption = caption_with(r, word)
    if not r.check(f'a caption shows the misheard "{word}"', caption, caption_texts(r)):
        return

    # What the preview shows at the caption's middle before the correction.
    r.seek(caption['startUs'] + caption['durationUs'] // 2)
    time.sleep(1.5)
    r.shot('caption-before')
    before = preview_crop(r.work / 'caption-before.png', preview_rect(r))

    double_click(r, target)
    opened = wait(lambda: field(r), 5)
    r.check(f'double-clicking "{word}" opens it for typing', opened and opened['value'] == word and opened['focused'], opened)
    type_keys(r, right)
    r.check('typing replaces the word', (field(r) or {}).get('value') == right, field(r))
    r.shot('word-editing')
    type_keys(r, ENTER)
    fixed = lambda: next((w for w in shown(r) if w['t'] == target), {})
    r.check(f'the transcript reads "{right}", underlined with dots, and says what was recognised',
            wait(lambda: fixed().get('text') == right, 10) and fixed().get('title') == f'Recognised as “{word}”'
            and fixed().get('underline') == 'dotted', fixed())
    r.check('the caption shows the corrected word', wait(lambda: caption_with(r, right) and not caption_with(r, word), 10),
            caption_texts(r))
    toast = next((t['text'] for t in r.state()['toasts'] if t['text'].startswith('Corrected')), None)
    r.check('a toast says what changed', toast == f'Corrected “{word}” to “{right}”, also in its caption', r.state()['toasts'])
    r.shot('word-corrected')
    _, redrawn = preview_redraw(r, 'caption-corrected', before)
    r.check('the preview draws the corrected caption', redrawn, 'the preview stayed the same')
    keep = caption_texts(r)
    r.check('the saved project holds the correction and the caption',
            wait(lambda: saved(r) == ([{'original': word, 'text': right}], keep), 10), saved(r))

    # One undo takes back the word and its caption together; redo brings both.
    r.s.run("[...document.querySelectorAll('[data-toast] button')].find((b) => b.textContent.trim() === 'Undo').click()")
    r.check('Undo in the toast takes back the word and its caption together',
            wait(lambda: fixed().get('text') == word and caption_with(r, word) and not caption_with(r, right), 10),
            {'word': fixed(), 'captions': caption_texts(r)})
    r.s.run('document.activeElement?.blur()')
    r.key('z', ctrlKey=True, shiftKey=True)
    r.check('redo brings both back', wait(lambda: fixed().get('text') == right and caption_texts(r) == keep, 10),
            caption_texts(r))

    job = generate_captions(r)
    r.check('regenerating the captions keeps the corrected word',
            job and job['status'] == 'done' and caption_with(r, right) and not caption_with(r, word), caption_texts(r))
    r.check('and saves it', wait(lambda: saved(r)[1] == caption_texts(r), 10), saved(r)[1])

    # Reopening the app: the same project, with the word and its caption as they were.
    close_window()
    r.check('the window closes', wait(lambda: not alive(r), 15))
    r.s.close()
    r.s = Session()
    r.check('the app starts again with the project',
            wait(lambda: r.s.run('return !!window.__capopen?.store.getState().snap', retries=3), 60)
            and wait(lambda: caption_with(r, right), 10), caption_texts(r))
    tab(r, 'transcript')
    r.check('after reopening, the transcript still reads the correction',
            wait(lambda: fixed().get('text') == right and fixed().get('title') == f'Recognised as “{word}”', 30), fixed())
    r.shot('reopened')

    agent(r, right, rest)
    r.check('no error toast', not r.errors(), r.errors())


def agent(r, first, rest):
    """The agent corrects another misheard word over MCP; captions built again keep both corrections."""
    if not r.check('recognition misheard a second word for the agent', rest, rest):
        return
    _, word, right = rest[0]
    bridge = Bridge(r, r.saved_project())
    try:
        run = bridge.call('begin_run', {'label': 'Fix words'})['run_id']
        transcript = bridge.call('get_transcript', {})
        i = next(w['i'] for w in transcript['words'] if w['text'] == word)
        stale = bridge.rpc('tools/call', {'name': 'correct_words', 'arguments': {
            'run_id': run, 'transcript_key': 'stale', 'corrections': [{'i': i, 'text': right}]}})
        r.check('a stale transcript key is refused', 'SPEECH_CHANGED' in json.dumps(stale), stale)
        fixed = bridge.call('correct_words', {'run_id': run, 'request_id': 'fix', 'transcript_key': transcript['transcript_key'],
                                              'corrections': [{'i': i, 'text': right}]})
        r.check(f'correct_words changes "{word}" to "{right}" and its caption',
                fixed['words'] == [{'i': i, 'before': word, 'after': right}] and len(fixed['captions_changed']) == 1, fixed)
        built = bridge.call('build_captions', {'run_id': run})
        r.check('build_captions afterwards keeps both corrections', built['caption_count'] > 0
                and caption_with(r, right) and caption_with(r, first) and not caption_with(r, word), caption_texts(r))
        again = bridge.call('get_transcript', {})
        r.check('get_transcript reads both corrections', again['words'][i]['text'] == right
                and again['transcript_key'] == fixed['transcript_key'] and any(w['text'] == first for w in again['words']),
                [w['text'] for w in again['words']])
        bridge.call('end_run', {'run_id': run, 'action': 'keep'})
        r.check('the lock ends with the run', wait(lambda: not r.state()['aiRun'], 10))
        r.check("the transcript in the app shows the agent's correction",
                wait(lambda: any(w['text'] == right for w in shown(r)), 10), shown(r))
        caption = caption_with(r, right)
        r.seek(caption['startUs'] + caption['durationUs'] // 2)
        time.sleep(1.5)
        r.shot('agent-corrected')
        r.s.run('document.activeElement?.blur()')
        r.key('z', ctrlKey=True)
        r.check("one undo takes back the agent's run, word and captions",
                wait(lambda: caption_with(r, word) and not caption_with(r, right)
                     and any(w['text'] == word for w in shown(r)), 10) and caption_with(r, first), caption_texts(r))
    finally:
        bridge.close()
