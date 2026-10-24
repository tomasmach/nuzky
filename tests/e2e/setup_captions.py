"""Get set up's Captions model step on an empty install: the spoken language starts as the system's, three models
with their size, state and the recommended one, and Download gets everything the first transcript in that language
needs, with the percentage, Stop and resuming, Try again after a network failure and a free space check. The choice
and the step stay after a restart, and the first captions download nothing more. On the processor, where the recommended
model depends on the language, the chosen model follows the language while it is the recommended one, unless it is
downloaded and at least as big. Czech is only the test's system language here."""
import json, time

from e2e.harness import FIXTURES, MACOS, MODELS, WINDOWS, flow, press, wait
from e2e.home import focus_card, make, resize, wait_library
from e2e.setup import BOX, CLICK, FOCUS_TEXT, relaunch
from e2e.speech_model import Models

SMALL, TURBO, VAD = 'ggml-small.bin', 'ggml-large-v3-turbo-q5_0.bin', 'ggml-silero-v5.1.2.bin'
CS_TIMING, EN_TIMING = 'wav2vec2-xls-r-300m-cs-250-q8_0.gguf', 'wav2vec2-base-960h.gguf'
# The addresses the app asks Hugging Face for, as in src-tauri/src/jobs.rs and ALIGN_MODELS in crates/analysis/src/align.rs.
CS_TIMING_PATH = f'/cstr/wav2vec2-xls-r-300m-cs-250-GGUF/resolve/a264af811793d1f32a6ff3ce7de7ad9b2dcfdd40/{CS_TIMING}'
EN_TIMING_PATH = f'/cstr/wav2vec2-base-960h-GGUF/resolve/7c025ea07cffc65b211b360e5865dfe59df7e8af/{EN_TIMING}'
SMALL_PATH = f'/ggerganov/whisper.cpp/resolve/main/{SMALL}'
PATHS = {SMALL_PATH: MODELS / SMALL,
         f'/ggerganov/whisper.cpp/resolve/main/{TURBO}': MODELS / TURBO,
         f'/ggml-org/whisper-vad/resolve/main/{VAD}': MODELS / VAD,
         CS_TIMING_PATH: MODELS / CS_TIMING,
         EN_TIMING_PATH: MODELS / EN_TIMING}
FREE_MB = 100
STOP = 'Stop downloading the speech model'
STEP = """const d = document.querySelector('[aria-labelledby=setup-title]');
if (!d) return null;
const panel = d.querySelector('[role=tabpanel]'), f = document.activeElement;
const lines = (el) => el.innerText.split('\\n').map((l) => l.trim()).filter((l) => l && !/^\\d$/.test(l));
return {
  panel: panel.querySelector('h3')?.textContent ?? null,
  language: panel.querySelector('select')?.value ?? null,
  cards: [...panel.querySelectorAll('[role=radiogroup] label')].map((l) => ({id: l.querySelector('input').value,
    checked: l.querySelector('input').checked, lines: lines(l)})),
  text: panel.innerText, status: panel.querySelector('[role=status]')?.innerText ?? null,
  alert: panel.querySelector('[role=alert]')?.innerText ?? null,
  buttons: [...d.querySelectorAll('footer button:not([data-sample])')].map((b) => b.textContent.trim()),
  rail: [...d.querySelectorAll('[role=tablist] [role=tab]')].map(lines), count: d.querySelector('#setup-count').textContent,
  focus: f?.getAttribute('role') === 'tab' ? 'tab' : f?.tagName === 'INPUT' ? 'radio ' + f.value : f?.tagName === 'SELECT' ? 'select'
    : f?.textContent.trim() || f?.getAttribute('aria-label') || f?.tagName,
};"""
# The footer and the chosen model in view inside the dialog, and the dialog inside the window.
FITS = """const d = document.querySelector('[aria-labelledby=setup-title]'), b = d.getBoundingClientRect();
const inside = (el) => { const e = el.getBoundingClientRect(); return e.width > 0 && e.left >= b.left && e.right <= b.right && e.top >= b.top && e.bottom <= b.bottom; };
const out = [...d.querySelectorAll('footer button'), d.querySelector('[role=radiogroup] label:has(input:checked)')].filter((el) => !inside(el)).map((el) => el.innerText);
if (b.top < 0 || b.bottom > innerHeight || b.left < 0 || b.right > innerWidth) out.push('the dialog leaves the window');
return out;"""
SET_LANGUAGE = """const s = document.querySelector('[aria-labelledby=setup-title] select');
s.value = arguments[0]; s.dispatchEvent(new Event('change', {bubbles: true}));"""
SETUP = "window.__nuzky.api.speechSetup(arguments[0]).then((s) => JSON.stringify(s))"
CHOSEN = "return window.__nuzky.speech.getState().model"


def mb(*names):
    """What Download offers for these files: each rounded on its own as the step lists it, at least 1 MB."""
    return sum(max(1, round((MODELS / name).stat().st_size / 1048576)) for name in names)


def before(r):
    (r.work / 'data/nuzky/models').mkdir(parents=True)
    r.models = Models(PATHS)
    r.env.update(NUZKY_MODEL_URL=f'http://127.0.0.1:{r.models.http.server_port}', NUZKY_TEST_FREE_MB=str(FREE_MB),
                 LANGUAGE='cs', LANG='cs_CZ.UTF-8')
    if MACOS:
        r.args = ['-AppleLanguages', '(cs)']
    if WINDOWS:
        # WebView2 otherwise takes the display language of Windows.
        r.env['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = '--lang=cs'


def step(r):
    return r.s.run(STEP, retries=3)


def job(r):
    return r.s.run("return window.__nuzky.store.getState().jobs['speech-model:setup'] ?? null")


def files(r):
    """Every file in the models folder with its size and when it last changed."""
    folder = r.work / 'data/nuzky/models'
    return {p.name: (p.stat().st_size, p.stat().st_mtime_ns) for p in sorted(folder.iterdir()) if not p.name.endswith('.lock')}


def card(d, model):
    return next((c for c in (d or {}).get('cards', []) if c['id'] == model), None)


def marked(d):
    return [c['id'] for c in (d or {}).get('cards', []) if 'Recommended' in c['lines']]


def small_window(r, name):
    """Screenshots the step in the smallest window and returns what of it does not fit."""
    if not resize(r, 1024, 640):
        return ['the window did not resize']
    time.sleep(0.5)
    out = r.s.run(FITS)
    r.shot(name)
    resize(r, 1440, 900)
    time.sleep(0.3)
    return out


def open_setup(r, first_start):
    if first_start:
        r.s.run(FOCUS_TEXT, 'Get set up', 'main')
        press('Return')
    else:
        r.s.run(CLICK, 'Get set up', 'nav[aria-label=Collections]')
    return wait(lambda: (d := step(r)) and d['cards'] and d, 10)


@flow('setup_captions', "Get set up's Captions model step downloads what the first transcript needs, with Stop, resuming, "
      'Try again and a free space check, and keeps the choice after a restart; the first captions download nothing',
      before=before, home=True)
def setup_captions(r):
    try:
        run(r)
    finally:
        r.models.http.shutdown()
        r.models.http.server_close()


def run(r):
    d = open_setup(r, first_start=True)
    if not r.check('Get set up opens on Captions model with three models', d and d['panel'] == 'Captions model'
                   and [c['lines'][0] for c in d['cards']] == ['Fast', 'Balanced', 'Most accurate'], d):
        return
    r.check('opening puts the focus on the step in the rail, not on Download or Skip', d['focus'] == 'tab', d['focus'])
    r.check('the spoken language starts as the system language, Czech', d['language'] == 'cs', d['language'])
    sizes = [next((l for l in c['lines'] if l.endswith(' MB')), None) for c in d['cards']]
    r.check('each model shows the size of its file and that it is not downloaded', sizes == ['141 MB', f'{mb(SMALL)} MB', f'{mb(TURBO)} MB']
            and all('Not downloaded' in c['lines'] for c in d['cards']), d['cards'])
    gpu = json.loads(r.s.call(SETUP, 'cs')['value'])['gpu']
    recommended = 'large-v3-turbo-q5_0' if gpu else 'small'
    marked = [c['id'] for c in d['cards'] if 'Recommended' in c['lines']]
    r.check(f'Recommended marks the model for recognition on the {"GPU" if gpu else "processor"}, and the step starts on it',
            marked == [recommended] and card(d, recommended)['checked']
            and r.s.run("return window.__nuzky.speech.getState().model") == recommended, {'gpu': gpu, 'cards': d['cards']})
    r.check('the step says where speech is recognised', ('graphics card' if gpu else 'processor') in d['text'], d['text'])
    lines = {c['id']: c['lines'] for c in d['cards']}
    r.check('each model says how many words it gets wrong in Czech, and the smaller ones how much quicker they are',
            f'66% of words wrong · {8 if gpu else 11}× quicker' in lines['base'] and '38% of words wrong · 4× quicker' in lines['small']
            and '11% of words wrong' in lines['large-v3-turbo-q5_0'], lines)
    file, label = (TURBO, 'Most accurate') if recommended == 'large-v3-turbo-q5_0' else (SMALL, 'Balanced')
    full = mb(file, VAD, CS_TIMING)
    r.check(f'in Czech Download gets the word timing model too, {full} MB, and the line above it adds up to that size',
            d['buttons'] == [f'Download {full} MB', 'Skip'] and 'Czech has a word timing model, so cuts land between words.' in d['text']
            and f'Download gets the {label} model ({mb(file)} MB), the Czech word timing model ({mb(CS_TIMING)} MB), and the voice detector (1 MB).' in d['text'], d)
    time.sleep(0.4)
    r.shot('captions-step')
    out = small_window(r, 'captions-step-1024')
    r.check('in a 1024 x 640 window the chosen model and the footer stay in the dialog', out == [], out)

    r.s.run(SET_LANGUAGE, 'de')
    d = wait(lambda: (d := step(r)) and d['language'] == 'de' and d['buttons'][0] != f'Download {full} MB' and d, 5)
    without = mb(file, VAD)
    r.check('in German, which has no word timing model, one sentence says word times are estimates and Download leaves it out',
            d and 'Word times in German are estimates' in d['text'] and d['buttons'][0] == f'Download {without} MB'
            and f'Download gets the {label} model ({mb(file)} MB) and the voice detector (1 MB).' in d['text'], d)
    r.shot('captions-step-german')
    r.s.run(SET_LANGUAGE, 'cs')
    r.check('Czech is picked again', wait(lambda: (step(r) or {}).get('language') == 'cs', 5))
    r.s.run(f"document.querySelector('{BOX} [role=tab][aria-selected=true]').focus()")

    press('Tab')
    r.check('Tab goes from the rail to the spoken language', wait(lambda: step(r)['focus'] == 'select', 3), step(r)['focus'])
    press('Tab')
    r.check('then to the chosen model', wait(lambda: step(r)['focus'] == f'radio {recommended}', 3), step(r)['focus'])
    for _ in range(3):
        if step(r)['focus'] == 'radio small':
            break
        press('Up' if recommended == 'large-v3-turbo-q5_0' else 'Down')
        time.sleep(0.2)
    d = step(r)
    balanced = mb(SMALL, VAD, CS_TIMING)
    r.check('the arrow keys pick Balanced, which Captions and Transcript now use', card(d, 'small')['checked']
            and r.s.run("return window.__nuzky.speech.getState().model") == 'small' and d['buttons'][0] == f'Download {balanced} MB', d)
    press('Tab')
    r.check('Tab goes on to the footer, Try the sample video at its left first', wait(lambda: step(r)['focus'] == 'Try the sample video', 3), step(r)['focus'])
    press('Tab')
    r.check('then to Download', wait(lambda: step(r)['focus'] == f'Download {balanced} MB', 3), step(r)['focus'])
    press('Return')
    d = wait(lambda: (d := step(r)) and d['alert'] and d, 5)
    r.check(f'with {FREE_MB} MB free, Enter on Download says how much more space it needs and downloads nothing',
            d and f'Free up {balanced - FREE_MB} MB more' in d['alert'] and not job(r) and not r.models.requests and files(r) == {},
            {'step': d, 'requests': r.models.requests, 'files': files(r)})
    r.shot('no-space')

    r.env.pop('NUZKY_TEST_FREE_MB')
    relaunch(r)
    d = open_setup(r, first_start=True)
    r.check('after a restart the step is still to do, in Czech and on Balanced', d and d['language'] == 'cs'
            and card(d, 'small')['checked'] and d['rail'][0] == ['Captions model'] and d['buttons'] == [f'Download {balanced} MB', 'Skip'], d)

    r.models.fail.add(SMALL_PATH)
    r.s.run(FOCUS_TEXT, 'Download', f'{BOX} footer')
    press('Return')
    r.check('when the network fails, the download fails', wait(lambda: (job(r) or {}).get('status') == 'failed', 30), job(r))
    d = wait(lambda: (d := step(r)) and d['alert'] and d['focus'] == 'Try again' and d, 5)
    r.check('the step says the speech model could not be downloaded and offers Try again, which takes the focus',
            d and 'speech model could not be downloaded' in d['alert'] and d['buttons'] == ['Try again', 'Skip'], step(r))
    r.check('no toast repeats the error over Get set up', not r.errors(), r.errors())
    r.shot('failed')

    r.models.rate = 40 * 1024 * 1024
    press('Return')
    r.check('Enter on Try again downloads, with the percentage', wait(lambda: (j := job(r)) and j['status'] == 'running' and j['progress'] >= 0.01, 15), job(r))
    d = wait(lambda: (d := step(r)) and d['focus'] == 'Stop' and d, 3)
    r.check('the step shows the file being downloaded, the percentage and Stop, which takes the focus',
            d and 'Downloading speech model' in (d['status'] or '') and '%' in d['status'], step(r))
    r.check('the top bar shows the same download with its Stop', r.s.run("""const h = document.querySelector('header');
        return h.innerText.includes('Downloading speech model') && !!h.querySelector('button[aria-label="Stop downloading the speech model"]');"""))
    r.shot('downloading')
    out = small_window(r, 'downloading-1024')
    r.check('in a 1024 x 640 window the download fits too', out == [], out)
    started = time.monotonic()
    press('Return')
    r.check('Enter on Stop ends the download within three seconds', wait(lambda: (job(r) or {}).get('status') == 'cancelled', 3, 0.1)
            and time.monotonic() - started <= 3, job(r))
    part = r.work / 'data/nuzky/models' / (SMALL + '.part')
    r.check('a partial file stays to resume', part.exists() and 0 < part.stat().st_size < (MODELS / SMALL).stat().st_size)
    d = wait(lambda: (d := step(r)) and d['buttons'][0] == 'Resume download' and d['focus'] == 'Resume download' and d, 5)
    r.check('the step says it stopped and offers Resume download, which takes the focus', d and 'Stopped' in (d['status'] or ''), step(r))
    r.s.run(SET_LANGUAGE, 'de')
    left = round((MODELS / SMALL).stat().st_size / 1048576 - part.stat().st_size / 1048576)
    rest = f'Download gets the rest of the Balanced model ({left} MB) and the voice detector (1 MB).'
    d = wait(lambda: (d := step(r)) and rest in d['text'] and d, 5)
    r.check('in another language the line says only the rest of Balanced is left, and adds up to Download',
            d and d['buttons'][0] == f'Download {left + 1} MB', step(r))
    r.s.run(SET_LANGUAGE, 'cs')
    r.check('back in Czech it offers Resume download again', wait(lambda: (d := step(r)) and d['buttons'][0] == 'Resume download', 5), step(r))
    r.s.run(FOCUS_TEXT, 'Resume download', f'{BOX} footer')

    r.models.rate = 60 * 1024 * 1024
    press('Return')
    r.check('Enter on Resume download goes on', wait(lambda: (job(r) or {}).get('status') == 'running', 10), job(r))
    resumed = wait(lambda: [q for q in r.models.requests if q['path'] == SMALL_PATH and q['range']], 10)
    r.check('the speech model goes on with a Range request from where it stopped', resumed and int(resumed[-1]['range'][6:-1]) > 0, r.models.requests)
    r.s.run(FOCUS_TEXT, 'Skip', f'{BOX} footer')
    press('Return')
    d = wait(lambda: (d := step(r)) and d['panel'] == 'Face and person models' and d, 5)
    r.check('Skip during the download marks the step Skipped and shows the next one', d and d['rail'][0] == ['Captions model', 'Skipped']
            and (job(r) or {}).get('status') == 'running', {'step': d, 'job': job(r)})
    r.check('the download goes on, and once it is done Captions model is done', wait(lambda: (job(r) or {}).get('status') == 'done', 120)
            and wait(lambda: ((step(r) or {}).get('rail') or [[]])[0] == ['Captions model', 'Done'], 5), {'step': step(r), 'job': job(r)})
    got = files(r)
    r.check('the speech model, the voice detector and the Czech word timing model are complete, with no partial file',
            {name: size for name, (size, _) in got.items()} == {n: (MODELS / n).stat().st_size for n in (SMALL, VAD, CS_TIMING)}, got)
    r.s.run(f"document.querySelector('{BOX} #setup-tab-captions').click()")
    d = wait(lambda: (d := step(r)) and d['panel'] == 'Captions model' and d['cards'] and d, 5)
    r.check('Captions model shows Balanced downloaded, nothing left to download, the header counts it and Next follows',
            d and d['count'] == '1 of 4 done' and d['buttons'] == ['Next'] and 'Downloaded' in card(d, 'small')['lines'], d)
    r.shot('done')

    r.s.run(SET_LANGUAGE, 'en')
    d = wait(lambda: (d := step(r)) and d['buttons'][0] == f'Download {mb(EN_TIMING)} MB' and d, 5)
    r.check('in English only its word timing model is left to download, and the line says so',
            d and f'Download gets the English word timing model ({mb(EN_TIMING)} MB).' in d['text'], step(r))
    r.models.fail.add(EN_TIMING_PATH)
    r.models.fail_delay = 3
    r.s.run(CLICK, 'Download', f'{BOX} footer')
    r.check('it starts', wait(lambda: (job(r) or {}).get('status') == 'running', 5), job(r))
    r.s.run(CLICK, 'Next', f'{BOX} footer')
    r.check('Next shows Face and person models while it downloads', wait(lambda: (step(r) or {}).get('panel') == 'Face and person models', 3), step(r))
    r.check('it fails there', wait(lambda: (job(r) or {}).get('status') == 'failed', 15), job(r))
    errors = wait(lambda: r.errors(), 3)
    r.check('with Captions model off screen, a toast says the word timing model could not be downloaded',
            errors and 'word timing model could not be downloaded' in errors[-1], errors)
    r.models.fail_delay = 0
    r.s.run("window.__nuzky.store.setState({toasts: []})")
    r.s.run(f"document.querySelector('{BOX} #setup-tab-captions').click()")
    r.check('Captions model shows the failure with Try again', wait(lambda: (d := step(r)) and d['cards'] and d['alert']
            and 'word timing model could not be downloaded' in d['alert'] and d['buttons'][0] == 'Try again', 5), step(r))
    r.s.run(SET_LANGUAGE, 'cs')
    r.check('Czech has nothing left to download', wait(lambda: (d := step(r)) and d['language'] == 'cs' and d['buttons'] == ['Next'], 5), step(r))

    make(r, 'reel.nuzky', 'First reel', [FIXTURES / 'first-reel.mov'], 0)
    downloaded, requests = files(r), len(r.models.requests)
    relaunch(r)
    wait_library(r)
    d = open_setup(r, first_start=False)
    r.check('after a restart Captions model is still done, on Balanced, with nothing to download', d and d['rail'][0] == ['Captions model', 'Done']
            and card(d, 'small')['checked'] and d['buttons'] == ['Next'] and r.s.run("return window.__nuzky.speech.getState().model") == 'small', d)
    press('Escape')
    r.check('Esc closes Get set up', wait(lambda: not step(r), 3))

    focus_card(r, 'First reel')
    press('Return')
    r.check('the project opens', wait(lambda: r.s.run("return window.__nuzky.store.getState().view") == 'editor', 20))
    r.s.run("[...document.querySelectorAll('[role=tab]')].find((t) => t.textContent.trim() === 'Captions').click()")
    picked = wait(lambda: r.s.run("""const f = [...document.querySelectorAll('label')].find((l) => l.textContent.startsWith('Accuracy'));
        return f?.querySelector('select')?.value;"""), 5)
    r.check('the Captions panel uses Balanced', picked == 'small', picked)
    r.s.run("[...document.querySelectorAll('button')].find((b) => b.textContent.includes('Generate captions')).click()")
    done = wait(lambda: next((j for j in r.state()['jobs'] if j['kind'] == 'captions' and j['status'] != 'running'), None), 300, 1)
    r.check('the first captions finish', done and done['status'] == 'done', done)
    texts = [c['text'] for t in r.state()['tracks'] if t['kind'] == 'text' for c in t['clips']]
    r.check('and are on the timeline', len(texts) > 0, texts[:8])
    r.check('they downloaded nothing: no request reached the model server and no model file changed',
            len(r.models.requests) == requests and files(r) == downloaded, {'requests': r.models.requests[requests:], 'files': files(r)})
    r.shot('first-captions')
    r.check('no error toast', not r.errors(), r.errors())
    on_processor(r)


def on_processor(r):
    """Recognition on the processor, as on a computer without a graphics card Nuzky can use: Recommended moves with the language, and the choice with it while it
    is the recommended one, so English on Fast becomes Czech on Balanced. A downloaded Balanced stays in English, where
    it is better than Fast, and a model picked over the recommended one stays."""
    r.env['NUZKY_TEST_SPEECH_CPU'] = '1'
    relaunch(r)
    wait_library(r)
    d = open_setup(r, first_start=False)
    r.check('on the processor, in Czech, Recommended marks Balanced, which is still chosen',
            d and 'recognises speech on its processor' in d['text'] and marked(d) == ['small'] and card(d, 'small')['checked'], d)
    r.s.run(SET_LANGUAGE, 'en')
    d = wait(lambda: (d := step(r)) and d['language'] == 'en' and marked(d) == ['base'] and d, 5)
    time.sleep(0.4)
    d = step(r)
    r.check('in English Recommended moves to Fast, and Balanced, downloaded and better in English too, stays chosen with no model to download',
            d and card(d, 'small')['checked'] and r.s.run(CHOSEN) == 'small' and d['buttons'][0] == f'Download {mb(EN_TIMING)} MB'
            and f'Download gets the English word timing model ({mb(EN_TIMING)} MB).' in d['text'], d)
    r.shot('processor-english-downloaded')
    r.s.run(SET_LANGUAGE, 'cs')
    r.check('back in Czech Balanced is chosen', wait(lambda: (d := step(r)) and d['language'] == 'cs' and card(d, 'small')['checked'], 5), step(r))
    pick = lambda model: r.s.run(f"document.querySelector('{BOX} input[value=\"{model}\"]').closest('label').click()")
    pick('large-v3-turbo-q5_0')
    r.check('a click picks Most accurate', wait(lambda: r.s.run(CHOSEN) == 'large-v3-turbo-q5_0', 3), r.s.run(CHOSEN))
    r.s.run(SET_LANGUAGE, 'en')
    d = wait(lambda: (d := step(r)) and d['language'] == 'en' and marked(d) == ['base'] and d, 5)
    time.sleep(0.4)
    r.check('in English Recommended moves to Fast, and Most accurate, picked over Balanced, stays chosen',
            d and card(step(r), 'large-v3-turbo-q5_0')['checked'] and r.s.run(CHOSEN) == 'large-v3-turbo-q5_0', step(r))
    pick('base')
    r.check('a click picks Fast', wait(lambda: r.s.run(CHOSEN) == 'base', 3), r.s.run(CHOSEN))

    relaunch(r)
    wait_library(r)
    d = open_setup(r, first_start=False)
    r.check('after a restart the step starts in English on Fast, which Recommended marks', d and d['language'] == 'en'
            and marked(d) == ['base'] and card(d, 'base')['checked'], d)
    r.shot('processor-english')
    r.s.run(SET_LANGUAGE, 'cs')
    d = wait(lambda: (d := step(r)) and d['language'] == 'cs' and marked(d) == ['small'] and d, 5)
    time.sleep(0.4)
    r.check('in Czech the choice moves to Balanced with Recommended, and Captions and Transcript use it',
            d and card(step(r), 'small')['checked'] and r.s.run(CHOSEN) == 'small', step(r))
    r.shot('processor-czech')
