"""Your style: Learn my style compares a recording with the creator's cut and suggests rules; Accept all writes the
same EDIT.md as `nuzky style learn` and the agent reads exactly it; the creator's own rules, a removed and a rejected
rule, an edit by hand, Back to default and Versions all change what the agent reads, and nothing else does."""
import json, subprocess, time

from pathlib import Path

from e2e.harness import CLI, FIXTURES, MODELS, Bridge, export, flow, link_models, wait
from e2e.home import CLICK_TEXT, TYPE, resize

FAKE = Path(__file__).resolve().parent / 'fake_agent'
# The AI beside the page; the editor's own panel stays mounted behind the home screen.
CHAT = 'main[aria-label="Your style"] ~ aside[aria-label=AI]'

# Sentences of the recording, whether the creator's cut keeps them, and the silence after them, as in
# crates/cli/tests/style.rs.
SCRIPT = [
    ("So today I want to", False, 0.8),
    ("So today I want to show you how I edit my videos.", True, 0.8),
    ("First I record", False, 0.7),
    ("First I record everything in one long take on my phone.", True, 0.9),
    ("Um.", False, 0.6),
    ("Then I remove the", False, 0.8),
    ("Then I remove the pauses and every slip of the tongue.", True, 1.0),
    ("By the way, my cat is sleeping next to me right now.", False, 1.2),
    ("Captions go on top, two words at a time.", True, 0.8),
    ("The zoom makes the important parts stand out.", True, 0.9),
    ("Music is optional and stays quiet under the voice.", True, 0.8),
    ("That is everything for today.", True, 1.0),
    ("Follow me and leave a comment below.", False, 0.5),
]
ZOOM = 1.25
OTHER = FIXTURES / 'reel-1.mp4'

VIEW = 'return window.__nuzky.style.getState().view'
JOB = """const l = window.__nuzky.style.getState().learning; if (!l) return null;
const j = window.__nuzky.store.getState().jobs[l.jobId]; return j && {status: j.status, phase: j.phase, results: l.results, message: j.message};"""
DIALOG = '[role=dialog][aria-labelledby=learn-title]'
PAGE = 'main[aria-label="Your style"]'


def run(*command, **kw):
    return subprocess.run(command, check=True, capture_output=True, **kw)


def duration(path):
    return float(run('ffprobe', '-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', str(path)).stdout)


def record(dir):
    """The recording, and the pieces of it the creator keeps."""
    parts = dir / 'parts'
    parts.mkdir(parents=True)
    listing, pieces, t = [], [], 0.0
    for i, (text, kept, pause) in enumerate(SCRIPT):
        wav, silence = parts / f's{i}.wav', parts / f'p{i}.wav'
        run('espeak-ng', '-v', 'en-us', '-s', '160', '-w', str(wav), text)
        run('ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', f'anullsrc=r=22050:cl=mono:d={pause}', str(silence))
        spoken = duration(wav)
        if kept:
            pieces.append((max(t - 0.06, 0.0), t + spoken + 0.1))
        t += spoken + pause
        listing += [f"file '{wav}'", f"file '{silence}'"]
    (parts / 'list.txt').write_text('\n'.join(listing) + '\n')
    speech, recording = dir / 'speech.wav', dir / 'talk.mp4'
    run('ffmpeg', '-v', 'error', '-y', '-f', 'concat', '-safe', '0', '-i', str(parts / 'list.txt'), str(speech))
    run('ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=s=540x960:r=30', '-i', str(speech), '-shortest',
        '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-b:a', '128k', str(recording))
    return recording, pieces


def pair(dir):
    """The recording and the creator's cut of it: retakes, a filler, a side remark and a call to action left out,
    every other piece zoomed in and two-word captions burned in."""
    recording, pieces = record(dir)
    cut = dir / 'reel.mp4'
    graph = ''
    for k, (start, end) in enumerate(pieces):
        zoom = f',crop=iw/{ZOOM}:ih/{ZOOM},scale=540:960' if k % 2 == 1 else ''
        graph += (f'[0:v]trim=start={start}:end={end},setpts=PTS-STARTPTS{zoom}[v{k}];'
                  f'[0:a]atrim=start={start}:end={end},asetpts=PTS-STARTPTS[a{k}];')
    length = sum(e - s for s, e in pieces)
    words = ['Hello there', 'big zoom', 'two words', 'short cut', 'fast pace', 'keep going', 'nice edit']
    stamp = lambda s: f'00:00:{int(s):02},{round(s % 1 * 1000) % 1000:03}'
    srt = ''.join(f'{i + 1}\n{stamp(i * 0.5)} --> {stamp(i * 0.5 + 0.5)}\n{words[i % len(words)]}\n\n' for i in range(int(length / 0.5)))
    (dir / 'captions.srt').write_text(srt)
    inputs = ''.join(f'[v{k}][a{k}]' for k in range(len(pieces)))
    graph += (f"{inputs}concat=n={len(pieces)}:v=1:a=1[v][a];"
              f"[v]subtitles={dir / 'captions.srt'}:force_style='Fontsize=24,Outline=2,Shadow=0,Alignment=5'[out]")
    run('ffmpeg', '-v', 'error', '-y', '-i', str(recording), '-filter_complex', graph, '-map', '[out]', '-map', '[a]',
        '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-b:a', '128k', str(cut))
    return recording, cut


def setup(r):
    missing = [str(p) for p in (OTHER, MODELS / 'ggml-small.bin') if not p.exists()]
    if missing:
        raise RuntimeError(f'run scripts/fixtures.sh first, missing: {missing}')
    link_models(r)
    r.pair = pair(r.work / 'pair')
    # The fake stands in for Claude Code in the AI panel, so the flow spends nobody's subscription.
    r.env.update(NUZKY_AGENT_CLAUDE=str(FAKE), NUZKY_AGENT_CODEX=str(r.work / 'no-codex'), NUZKY_FAKE_AGENT_LOG=str(r.work / 'fake'))


def edit_md(r):
    path = r.work / 'data/nuzky/EDIT.md'
    return path.read_text() if path.exists() else None


def agent_style(r):
    """What an agent connected over MCP reads as the creator's style, or None without one."""
    bridge = Bridge(r, r.saved_project())
    try:
        read = bridge.rpc('resources/read', {'uri': 'nuzky://style'})
        return read['result']['contents'][0]['text'] if 'result' in read else None
    finally:
        bridge.close()


def click(r, text, scope=PAGE):
    return r.s.run(CLICK_TEXT, text, scope)


def view(r):
    return r.s.run(VIEW)


def rule_menu(r, title, item):
    """Chooses `item` in the More menu of a learned rule."""
    r.s.run("document.querySelector(`button[aria-label='More for ${arguments[0]}']`).click()", title)
    wait(lambda: r.s.run("return !!document.querySelector('[role=menu]')"), 5)
    return r.s.run(CLICK_TEXT, item, '[role=menu]')


@flow('style', 'Your style: Learn my style suggests rules from a recording and its cut, Accept all writes what `nuzky style learn` '
               'writes and the agent reads it; own rules, Remove, Reject, editing as text, Back to default and Versions change '
               'what the agent reads', before=setup, home=True)
def style(r):
    recording, cut = r.pair
    r.s.run("document.querySelector('nav[aria-label=Collections] button[data-row]:nth-of-type(2)').click()")
    r.check('Your style opens from the sidebar', wait(lambda: r.s.run(f"return !!document.querySelector('{PAGE}')"), 10))
    r.check('without a style the page says how Nuzky learns', wait(lambda: r.s.run(f"return document.querySelector('{PAGE}').textContent.includes('Nuzky learns how you edit')"), 10))
    r.shot('empty')

    click(r, 'Learn my style')
    r.check('Learn my style opens its dialog on Videos', wait(lambda: r.s.run(f"return !!document.querySelector('{DIALOG}')"), 5))
    r.s.run("window.__nuzky.style.setState({draft: arguments[0]})",
            [{'recording': str(recording), 'cut': str(cut)}, {'recording': str(recording), 'cut': str(OTHER)}])
    r.shot('learn-ready')
    click(r, 'Learn', DIALOG)
    r.check('learning runs with its progress', wait(lambda: (j := r.s.run(JOB)) and j['status'] == 'running' and j['phase'], 20), r.s.run(JOB))
    time.sleep(2)
    r.shot('learning')
    r.check('learning ends', wait(lambda: (j := r.s.run(JOB)) and j['status'] != 'running', 600), r.s.run(JOB))
    job = r.s.run(JOB)
    first, second = job['results']
    r.check('the creator\'s cut matched its recording', job['status'] == 'done' and first and not first['error'] and first['matched'] >= 0.9, job)
    r.check('a video not cut from the recording is skipped and says why', second and second['error'] and 'NO_MATCH' in second['error'], second)
    shown = r.s.run(f"return document.querySelector('{DIALOG}').textContent")
    r.check('the dialog shows each pair\'s match', 'Matched' in shown and 'Skipped' in shown, shown)
    r.shot('learned')
    r.check('learning alone does not change the style', edit_md(r) is None and agent_style(r) is None)

    click(r, 'Review', DIALOG)
    r.check('suggestions with confidence and moments', wait(lambda: len((view(r) or {}).get('suggestions', [])) >= 5, 10), view(r))
    page = r.s.run(f"return document.querySelector('{PAGE}').textContent")
    r.check('each says in how many videos and moments it holds', 'in 1 of 1 video' in page and 'moments' in page, page[:500])
    r.s.run(f"[...document.querySelectorAll('{PAGE} button')].find((b) => b.textContent.startsWith('Show moments')).click()")
    r.shot('suggestions')
    count = r.s.run("return document.querySelector('nav[aria-label=Collections] button[data-row]:nth-of-type(2)').textContent")
    r.check('the sidebar counts the suggestions', str(len(view(r)['suggestions'])) in count, count)

    click(r, 'Accept all')
    r.check('Accept all writes the style', wait(lambda: edit_md(r) is not None, 10))
    cli = r.work / 'cli.md'
    run(str(CLI), 'style', 'learn', '--out', str(cli), str(recording), str(cut), env=r.env)
    r.check('it is byte for byte what nuzky style learn writes from the same videos', edit_md(r) == cli.read_text(),
            {'app': len(edit_md(r) or ''), 'cli': len(cli.read_text())})
    r.check('the agent reads exactly the style', agent_style(r) == edit_md(r))
    r.shot('accepted')

    rule = 'Never cut the sentence where I name the product.'
    r.s.run(TYPE, f'{PAGE} input[aria-label="Tell the AI what to do or not to do"]', rule)
    r.s.run("document.activeElement.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true}))")
    r.check('a rule in plain words is kept in the style', wait(lambda: f'- {rule}' in (edit_md(r) or ''), 10), edit_md(r))
    r.check('and the agent reads it', rule in (agent_style(r) or ''))

    # Talking it through with the AI: what they agree on shows on the page at once, with Undo.
    r.s.run("[...document.querySelectorAll('[aria-label=Projects] > header button')].find((b) => b.textContent.trim() === 'AI').click()")
    r.check('AI opens beside the style', wait(lambda: r.s.run(f"return !!document.querySelector('{CHAT}')"), 5))
    chip = r.s.run(f"return document.querySelector('{CHAT}').innerText")
    r.check('the message will be about the style, not the open video', 'Your style' in chip and 'Playhead' not in chip, chip)
    asked = 'Never cut my sign-off at the end.'
    r.s.run(f"""const t = document.querySelector('{CHAT} textarea'); t.focus();
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(t, arguments[0]);
        t.dispatchEvent(new Event('input', {{bubbles: true}}));""", f'keep: {asked}')
    r.s.run("document.activeElement.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true, cancelable: true}))")
    shown = f"return [...document.querySelectorAll('{PAGE} section[aria-label=\"Your rules\"] li')].map((l) => l.textContent)"
    r.check('the rule the AI kept shows on the page without reloading', wait(lambda: asked in (r.s.run(shown) or []), 20), r.s.run(shown))
    r.check('the changed rule is marked for a moment', r.s.run(f"return [...document.querySelectorAll('{PAGE} li.changed')].some((l) => l.textContent.includes(arguments[0]))", asked))
    r.check('it is in the style the agent reads', f'- {asked}' in (edit_md(r) or ''))
    note = "return window.__nuzky.store.getState().toasts.find((t) => t.text.startsWith('The AI changed your style'))"
    toast = wait(lambda: r.s.run(note), 5)
    r.check('a toast says the AI changed the style, with Undo', toast and toast['text'] == 'The AI changed your style: Added your rule' and toast['action']['label'] == 'Undo', toast)
    prompt = (r.work / 'fake/runs.jsonl').read_text()
    r.check('the agent was told the message is about the style', 'The user writes from the Your style page' in prompt)
    r.shot('chat')
    r.s.run("window.__nuzky.store.getState().toasts.find((t) => t.text.startsWith('The AI changed your style')).action.run()")
    r.check('Undo takes the AI\'s change back', wait(lambda: f'- {asked}' not in (edit_md(r) or ''), 10) and wait(lambda: asked not in r.s.run(shown), 5))
    r.s.run(f"document.querySelector('{CHAT} button[aria-label=\"Close AI\"]').click()")
    r.check('Close puts the AI away', wait(lambda: not r.s.run(f"return !!document.querySelector('{CHAT}')"), 5))

    rule_menu(r, 'Pauses', 'Remove')
    r.check('Remove takes a learned rule out', wait(lambda: '## Pauses' not in (edit_md(r) or '## Pauses'), 10))
    r.check('and its setting', 'shorten_pauses_us' not in edit_md(r))
    r.check('learning leaves it out', any(n['title'] == 'Pauses' for n in view(r)['notLearned']) and not view(r)['suggestions'])
    r.s.run(f"[...document.querySelectorAll('{PAGE} button')].find((b) => b.textContent.startsWith('Not learned')).click()")
    click(r, 'Learn again')
    r.check('Learn again suggests it again', wait(lambda: [s['title'] for s in view(r)['suggestions']] == ['Pauses'], 10), view(r))
    click(r, 'Reject')
    r.check('a rejected suggestion stays out of the style', wait(lambda: not view(r)['suggestions'], 10) and '## Pauses' not in edit_md(r))
    r.check('the agent never sees a rejected or removed rule', '## Pauses' not in agent_style(r) and rule in agent_style(r))
    r.shot('rules')

    before = edit_md(r)
    click(r, 'Edit as text')
    r.check('Edit as text shows the whole file', wait(lambda: r.s.run("return document.querySelector('textarea[aria-label=\"EDIT.md\"]')?.value") == before, 5))
    write = """const el = document.querySelector('textarea[aria-label="EDIT.md"]');
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(el, arguments[0]);
        el.dispatchEvent(new Event('input', {bubbles: true}));"""
    r.s.run(write, before.replace('Cut every earlier attempt', 'Cut each earlier attempt'))
    r.shot('edit-as-text')
    # Meanwhile an agent in a terminal keeps a rule: saving the text opened before must not take it away.
    bridge = Bridge(r, r.saved_project())
    try:
        bridge.call('change_style', {'action': {'type': 'setOwn', 'text': 'Keep my intro.'}})
    finally:
        bridge.close()
    r.check('the page learns of a change made elsewhere', wait(lambda: 'Keep my intro.' in view(r)['own'], 5))
    r.s.run("""document.querySelector('textarea[aria-label="EDIT.md"]').dispatchEvent(
        new KeyboardEvent('keydown', {key: 's', ctrlKey: true, bubbles: true, cancelable: true}))""")
    conflict = "return window.__nuzky.store.getState().toasts.find((t) => t.kind === 'error' && t.text.startsWith('Your style changed'))?.text ?? null"
    r.check('saving over it says the style changed meanwhile', wait(lambda: r.s.run(conflict), 5), r.state()['toasts'])
    r.check('and keeps both the change made meanwhile and the typed text', '- Keep my intro.' in edit_md(r)
            and r.s.run("return document.querySelector('textarea[aria-label=\"EDIT.md\"]').value").count('Cut each earlier attempt') == 1)
    r.s.run("window.__nuzky.store.getState().toasts.filter((t) => t.kind === 'error').forEach((t) => window.__nuzky.store.getState().dismissToast(t.id))")
    click(r, 'Cancel')
    before = edit_md(r)
    click(r, 'Edit as text')
    wait(lambda: r.s.run("return document.querySelector('textarea[aria-label=\"EDIT.md\"]')?.value") == before, 5)
    changed = before.replace('Cut every earlier attempt', 'Cut each earlier attempt')
    r.s.run(write, changed)
    r.s.run("""document.querySelector('textarea[aria-label="EDIT.md"]').dispatchEvent(
        new KeyboardEvent('keydown', {key: 's', ctrlKey: true, bubbles: true, cancelable: true}))""")
    r.check('Ctrl+S saves the text as written', wait(lambda: edit_md(r) == changed, 10), edit_md(r))
    r.check('the rule changed by hand is marked and learning leaves it alone',
            wait(lambda: any(x['title'] == 'Restarted sentences' and x['byYou'] for x in view(r)['rules']), 5), view(r)['rules'])

    r.s.run(f"document.querySelector('{PAGE} button[aria-label=\"Versions of your style\"]').click()")
    r.check('Versions lists every change', wait(lambda: r.s.run("return [...document.querySelectorAll('[role=menu] [role=menuitemradio]')].length") >= 6, 5))
    r.shot('versions')
    r.s.run(CLICK_TEXT, 'Back to default', '[role=menu]')
    r.check('Back to default asks first', wait(lambda: r.s.run("return !!document.querySelector('[role=alertdialog]')"), 5))
    r.shot('reset')
    r.s.run(CLICK_TEXT, 'Back to default', '[role=alertdialog]')
    r.check('Back to default removes the style the agent reads', wait(lambda: edit_md(r) is None, 10) and agent_style(r) is None)
    undo = "return window.__nuzky.store.getState().toasts.find((t) => t.text === 'Your style is the default now')?.action?.label ?? null"
    r.check('a toast offers Undo', r.s.run(undo) == 'Undo')
    r.s.run("window.__nuzky.store.getState().toasts.find((t) => t.text === 'Your style is the default now').action.run()")
    r.check('Undo brings the style back as it was', wait(lambda: edit_md(r) == changed, 10) and agent_style(r) == changed)

    if r.check('the window goes to its smallest size', resize(r, 1024, 640)):
        r.shot('style-1024')
        width = r.s.run(f"const m = document.querySelector('{PAGE}'); return [m.scrollWidth, m.clientWidth]")
        r.check('the page fits the smallest window', width[0] <= width[1], width)
        resize(r, 1440, 900)
    r.check('no error toast', not r.errors(), r.errors())
    versions = json.loads(json.dumps(view(r)['versions']))
    r.check('every change is a version', [v['label'] for v in versions][:3] == ['Restored: Edited by hand', 'Back to default', 'Edited by hand'], versions[:4])
    r.check('the AI\'s change is a version of its own', any(v['label'] == 'AI: Added your rule' for v in versions), versions)


def project_setup(r):
    missing = [str(p) for p in (MODELS / 'ggml-small.bin',) if not p.exists()]
    if missing:
        raise RuntimeError(f'run scripts/fixtures.sh first, missing: {missing}')
    link_models(r)
    recording, _ = record(r.work / 'pair')
    run(str(CLI), 'new', str(r.work / 'data/nuzky/projects/talk.nuzky'), str(recording), env=r.env)


def poll(bridge, job, timeout):
    return wait(lambda: (s := bridge.call('job', {'job_id': job['job_id'], 'action': 'get'}))['status'] != 'running' and s, timeout, 0.5)


def answer(r, expression, *args):
    """What an app call answers, an object too, which Session.call does not hand back."""
    done = r.s.call(f"({expression}).then((v) => {{ window.__flowAnswer = v; return 1; }})", *args)
    if not done['ok']:
        raise RuntimeError(done['error'])
    return r.s.run('return window.__flowAnswer')


def sources(r):
    return (view(r) or {}).get('sources', [])


@flow('style_auto', 'Nuzky learns by itself: never from the AI\'s own cut or its export, but from the user\'s edits of it, their corrected '
                    'words and their export, all as suggestions that leave the style unchanged', before=project_setup, home=True)
def style_auto(r):
    bridge = Bridge(r, r.saved_project())
    try:
        job = poll(bridge, bridge.call('transcribe', {'language': 'en', 'model': 'small'}), 300)
        r.check('the speech is recognised', job and job['status'] == 'done', job)
        words = bridge.call('get_transcript', {})['words']
        start = next(i for i, w in enumerate(words) if w['text'].strip().lower().startswith('show'))
        run = bridge.call('begin_run', {'label': 'Rough cut'})['run_id']
        # The first attempt at the opening sentence, up to where the retake starts.
        retake = next(i for i in range(1, start) if words[i]['text'].strip().lower() == 'so')
        bridge.call('apply_edits', {'run_id': run, 'request_id': 'cut', 'edits': [
            {'type': 'rippleDeleteRanges', 'ranges': [{'startUs': words[0]['start_us'], 'endUs': words[retake]['start_us']}]}]})
        bridge.call('end_run', {'run_id': run, 'action': 'keep'})
        exported = bridge.call('export_video', {'path': str(r.work / 'agent.mp4'), 'resolution': 720, 'fps': 30, 'quality': 'small'})
        r.check('the agent exports its cut', poll(bridge, exported, 180)['status'] == 'done')
    finally:
        bridge.close()
    time.sleep(12)
    r.check('neither the AI\'s cut nor its export teaches anything', sources(r) == [] and edit_md(r) is None, sources(r))

    # The user corrects the AI's cut: a side remark goes, and three misheard words are put right.
    epoch = "window.__nuzky.store.getState().snap.sessionEpoch"
    shown = answer(r, "window.__nuzky.api.transcriptView(700000)")
    texts = [w['text'].strip().lower().strip('.,') for w in shown['words']]
    remark = texts.index('by'), texts.index('now')
    r.s.call(f"window.__nuzky.api.cutWords(arguments[0], [arguments[1]], {epoch})", shown['key'], list(remark))
    shown = answer(r, "window.__nuzky.api.transcriptView(700000)")
    texts = [w['text'].strip().lower().strip('.,') for w in shown['words']]
    # As typed in the transcript: the recognised punctuation stays.
    said = [w['text'].strip() for w in shown['words']]
    trail = lambda word: word[len(word.rstrip('.,')):]
    fixes = [{'i': (i := texts.index(word)), 'text': right + trail(said[i])}
             for word, right in (('phone', 'iPhone'), ('videos', 'reels'), ('captions', 'subtitles'))]
    corrected = r.s.call(f"window.__nuzky.api.correctWords(arguments[0], arguments[1], {epoch})", shown['key'], fixes)
    r.check('the user cuts a side remark and corrects three words', corrected['ok'], corrected)
    r.check('10 s after the last edit Nuzky learns from it', wait(lambda: [s['kind'] for s in sources(r)] == ['project'], 25), sources(r))
    titles = [s['title'] for s in view(r)['suggestions']]
    r.check('it suggests rules, the corrected words among them', 'Spelling' in titles, titles)
    spelling = next(s for s in view(r)['suggestions'] if s['title'] == 'Spelling')
    r.check('Spelling quotes the corrections', any(m['line'].startswith('"phone') and '"iPhone' in m['line'] for m in spelling['moments']), spelling['moments'])
    r.check('and the style is unchanged until accepted', edit_md(r) is None)
    note = "return window.__nuzky.store.getState().toasts.find((t) => t.text.startsWith('Learned from')) ?? null"
    toast = wait(lambda: r.s.run(note), 5)
    r.check('a toast says what was learned, with Review', toast and toast['action']['label'] == 'Review', toast)
    r.s.run("window.__nuzky.store.getState().toasts.find((t) => t.text.startsWith('Learned from')).action.run()")
    r.check('Review shows the suggestions', wait(lambda: r.s.run(f"return !!document.querySelector('{PAGE}')"), 5))
    r.shot('learned-by-itself')

    learned_at = sources(r)[0]['atMs']
    bridge = Bridge(r, r.saved_project())
    try:
        run = bridge.call('begin_run', {'label': 'Discarded'})['run_id']
        bridge.call('apply_edits', {'run_id': run, 'request_id': 'cut', 'edits': [{'type': 'rippleDeleteRanges', 'ranges': [{'startUs': 0, 'endUs': 2_000_000}]}]})
        bridge.call('end_run', {'run_id': run, 'action': 'discard'})
    finally:
        bridge.close()
    time.sleep(12)
    r.check('a discarded AI run teaches nothing', sources(r)[0]['atMs'] == learned_at, sources(r))

    export(r, 'mine.mp4')
    r.check('the user\'s export is learned again', wait(lambda: sources(r)[0]['atMs'] > learned_at, 10), sources(r))
    r.check('as the same video, not a second one', len(sources(r)) == 1, sources(r))
    r.check('no error toast', not r.errors(), r.errors())
