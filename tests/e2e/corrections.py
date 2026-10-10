"""Correcting what Whisper misheard in a Czech take: the user types the word in the transcript and the caption
changes with it; generating the captions again, reopening the app and undo all keep the word and its caption
together. Then an agent does the same over MCP."""
import difflib, json, time

from e2e.harness import (FIXTURES, MACOS, MODELS, Bridge, Session, close_window, flow, link_models, pointer, press,
                         preview_crop, preview_rect, preview_redraw, wait)
from e2e.reel import phrases, words as plain

TAKE = FIXTURES / 'reel-2.mp4'
MODEL = 'large-v3-turbo-q5_0'
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


class Hands:
    """A user's mouse and keyboard: real input into the app window, through the X server (XTEST) under gamescope
    and through the window itself on macOS, so the webview turns two clicks into a double-click and keys into
    text. WebKitWebDriver here has no Actions or element click."""

    KEYS = {'.': 'period', ',': 'comma', ' ': 'space', '\n': 'Return', '!': 'exclam', '?': 'question'}

    def __init__(self, r):
        self.r, self.at = r, (0, 0)
        if MACOS:
            # Clicks land at page coordinates, and WebKit ignores pointer moves made off screen: nothing to calibrate.
            self.origin, self.scale = (0, 0), 1
            return
        from Xlib import display
        self.d = display.Display()
        r.s.run("window.__pointerAt = null;"
                "addEventListener('pointermove', (e) => { window.__pointerAt = [e.clientX, e.clientY]; }, true);")
        self.calibrate()

    def calibrate(self):
        """Where the page sits on the screen and how large its pixels are, from two pointer positions."""
        (x0, y0), (x1, y1) = (500, 300), (900, 600)
        (c0, d0), (c1, d1) = self.read(x0, y0), self.read(x1, y1)
        self.scale = (x1 - x0) / (c1 - c0)
        self.origin = (x0 - c0 * self.scale, y0 - d0 * self.scale)

    def fake(self, kind, detail=0, **at):
        from Xlib.ext import xtest
        xtest.fake_input(self.d, kind, detail, **at)
        self.d.sync()

    def read(self, x, y):
        """Moves the pointer and returns where the page saw it last, once earlier moves have arrived."""
        from Xlib import X
        self.fake(X.MotionNotify, x=x, y=y)
        time.sleep(0.4)
        at = self.r.s.run('return window.__pointerAt')
        if not at:
            raise RuntimeError('the page did not see the pointer move')
        return at

    def over(self, css):
        """Moves the pointer to the middle of the element; true once the page sees it there."""
        left, top = self.r.s.run('const b = document.querySelector(arguments[0]).getBoundingClientRect();'
                                 'return [b.left + b.width / 2, b.top + b.height / 2];', css)
        self.at = (round(self.origin[0] + left * self.scale), round(self.origin[1] + top * self.scale))
        if MACOS:
            return self.r.s.run('return !!document.elementFromPoint(arguments[0], arguments[1])?.closest(arguments[2]);',
                                *self.at, css)
        self.read(*self.at)
        return self.r.s.run('const [x, y] = window.__pointerAt;'
                            'return !!document.elementFromPoint(x, y)?.closest(arguments[0]);', css)

    def double_click(self, css):
        """Two clicks on the element within the double-click time; false when the pointer missed it."""
        if not self.over(css):
            if MACOS:
                return False
            self.calibrate()
            if not self.over(css):
                return False
        for clicks in (1, 2):
            if MACOS:
                pointer('down', *self.at, clicks)
                time.sleep(0.03)
                pointer('up', *self.at, clicks)
            else:
                from Xlib import X
                self.fake(X.ButtonPress, 1)
                time.sleep(0.03)
                self.fake(X.ButtonRelease, 1)
            time.sleep(0.06)
        return True

    def type(self, text):
        if MACOS:
            for c in text:
                press({'\n': 'Return'}.get(c, c))
                time.sleep(0.02)
            return
        from Xlib import X, XK
        for c in text:
            keysym = XK.string_to_keysym(self.KEYS.get(c, c))
            code = self.d.keysym_to_keycode(keysym)
            if not code:
                raise RuntimeError(f'no key for {c!r} on this keyboard')
            shift = self.d.keycode_to_keysym(code, 0) != keysym
            if shift:
                self.fake(X.KeyPress, self.d.keysym_to_keycode(XK.XK_Shift_L))
            self.fake(X.KeyPress, code)
            self.fake(X.KeyRelease, code)
            if shift:
                self.fake(X.KeyRelease, self.d.keysym_to_keycode(XK.XK_Shift_L))
            time.sleep(0.02)


def field(r):
    return r.s.run("const f = document.querySelector('[role=listbox][aria-label=Transcript] input');"
                   "return f && {value: f.value, focused: document.activeElement === f, "
                   "selected: f.selectionStart === 0 && f.selectionEnd === f.value.length};")


def generate_captions(r):
    tab(r, 'captions')
    jobs = lambda: [j for j in r.state()['jobs'] if j['kind'] == 'captions']
    count = len(jobs())
    r.s.run("[...document.querySelectorAll('button')].find((b) => /enerate captions/.test(b.textContent)).click()")
    return wait(lambda: next((j for j in jobs()[count:] if j['status'] != 'running'), None), 600, 1)


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
    r.s.run("window.__nuzky.speech.setState({model: arguments[0], language: 'cs'})", MODEL)
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

    hands = Hands(r)
    r.check('the pointer reaches the word', hands.double_click(f'[data-t="{target}"]'),
            {'origin': hands.origin, 'scale': hands.scale})
    opened = wait(lambda: field(r), 5)
    r.check(f'double-clicking "{word}" opens it for typing, the whole word selected',
            opened and opened['value'] == word and opened['focused'] and opened['selected'], opened)
    hands.type(right)
    r.check('typing replaces the word', (field(r) or {}).get('value') == right, field(r))
    r.shot('word-editing')
    hands.type('\n')
    toasts = lambda: [t['text'] for t in r.state()['toasts']]
    toast = wait(lambda: next((t for t in toasts() if t.startswith('Corrected')), None), 5)
    r.check('a toast says what changed', toast == f'Corrected “{word}” to “{right}”, also in its caption', toasts())
    fixed = lambda: next((w for w in shown(r) if w['t'] == target), {})
    r.check(f'the transcript reads "{right}", underlined with dots, and says what was recognised',
            wait(lambda: fixed().get('text') == right, 5) and fixed().get('title') == f'Recognised as “{word}”'
            and fixed().get('underline') == 'dotted', fixed())
    r.check('the caption shows the corrected word', wait(lambda: caption_with(r, right) and not caption_with(r, word), 5),
            caption_texts(r))
    keep = caption_texts(r)
    r.shot('word-corrected')

    # One undo takes back the word and its caption together; redo brings both.
    undo = r.s.run("const b = [...document.querySelectorAll('[data-toast] button')].find((b) => b.textContent.trim() === 'Undo');"
                   "b?.click(); return !!b;")
    if not undo:
        r.s.run('document.activeElement?.blur()')
        r.key('z', ctrlKey=True)
    r.check('Undo in the toast takes back the word and its caption together',
            undo and wait(lambda: fixed().get('text') == word and caption_with(r, word) and not caption_with(r, right), 10),
            {'toast': undo, 'word': fixed(), 'captions': caption_texts(r)})
    r.s.run('document.activeElement?.blur()')
    r.key('z', ctrlKey=True, shiftKey=True)
    r.check('redo brings both back', wait(lambda: caption_texts(r) == keep and fixed().get('text') == right, 10),
            {'word': fixed(), 'captions': caption_texts(r)})
    _, redrawn = preview_redraw(r, 'caption-corrected', before)
    r.check('the preview draws the corrected caption', redrawn, 'the preview stayed the same')
    r.check('the saved project holds the correction and the caption',
            wait(lambda: saved(r) == ([{'original': word, 'text': right}], keep), 10), saved(r))

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
            wait(lambda: r.s.run('return !!window.__nuzky?.store.getState().snap', retries=3), 60)
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
