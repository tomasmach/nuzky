"""Learn my style from projects cut in another editor: an OpenTimelineIO file of a recording cut on frames is read
at once with what is missing and not read, learning runs as a job that Stop ends, the dialog shows EDIT.md as
`nuzky style learn --from` writes it, and the style changes only when the creator uses it; an existing style is
replaced only after they agree, keeping their own rules."""
import json, subprocess, time

from e2e.harness import AI_EDITING, CLI, MODELS, flow, link_models, wait
from e2e.home import CLICK_TEXT, TYPE, resize
from e2e.style import DIALOG, PAGE, duration, edit_md, record, view

PROJECTS = 'return window.__nuzky.style.getState().projects'
JOB = """const l = window.__nuzky.style.getState().projectsLearning; if (!l) return null;
const j = window.__nuzky.store.getState().jobs[l.jobId];
return j && {status: j.status, phase: j.phase, progress: j.progress, output: j.output, results: l.results, message: j.message};"""
DIALOG_TEXT = f"return document.querySelector('{DIALOG}')?.innerText ?? ''"
PREVIEW = f"return document.querySelector('{DIALOG} section[aria-label=\"EDIT.md as learned\"] pre')?.textContent ?? null"


def timeline(path, recording, pieces, name='Talking reel'):
    """The pieces as an editor exports them: the camera's picture on V1 and its sound on A1, on frames of 30 fps
    against the file's timecode of 01:00:00:00, the sound counted in samples, a gap after the second piece."""
    hour = 3600 * 30
    frames = lambda seconds: round(seconds * 30)
    rt = lambda n, audio: {'OTIO_SCHEMA': 'RationalTime.1', 'value': n * 1600 if audio else n, 'rate': 48000 if audio else 30}
    span = lambda start, length, audio: {'OTIO_SCHEMA': 'TimeRange.1', 'start_time': rt(start, audio), 'duration': rt(length, audio)}
    length = int(duration(recording) * 30)

    def clip(start, end, audio):
        return {'OTIO_SCHEMA': 'Clip.2', 'name': recording.name, 'source_range': span(hour + frames(start), frames(end - start), audio),
                'media_references': {'DEFAULT_MEDIA': {'OTIO_SCHEMA': 'ExternalReference.1', 'target_url': recording.as_uri(),
                                                       'available_range': span(hour, length, audio)}},
                'active_media_reference_key': 'DEFAULT_MEDIA'}

    def track(kind, audio):
        children = []
        for k, (start, end) in enumerate(pieces):
            children.append(clip(start, end, audio))
            if k == 1:
                children.append({'OTIO_SCHEMA': 'Gap.1', 'source_range': span(0, 15, audio)})
        return {'OTIO_SCHEMA': 'Track.1', 'name': kind, 'kind': kind, 'children': children}

    path.write_text(json.dumps({'OTIO_SCHEMA': 'Timeline.1', 'name': name, 'global_start_time': rt(hour, False),
                                'tracks': {'OTIO_SCHEMA': 'Stack.1', 'children': [track('Video', False), track('Audio', True)]}}))


def missing(path, folder):
    """A project shared by someone else: its recording is not on this computer and its B-roll is on the web."""
    def clip(url):
        return {'OTIO_SCHEMA': 'Clip.2', 'name': 'clip',
                'source_range': {'OTIO_SCHEMA': 'TimeRange.1', 'start_time': {'OTIO_SCHEMA': 'RationalTime.1', 'value': 0, 'rate': 25},
                                 'duration': {'OTIO_SCHEMA': 'RationalTime.1', 'value': 50, 'rate': 25}},
                'media_references': {'DEFAULT_MEDIA': {'OTIO_SCHEMA': 'ExternalReference.1', 'target_url': url}}}
    tracks = [{'OTIO_SCHEMA': 'Track.1', 'kind': 'Video', 'children': [clip((folder / 'their-talk.mp4').as_uri()), clip('https://example.com/broll.mp4')]}]
    path.write_text(json.dumps({'OTIO_SCHEMA': 'Timeline.1', 'name': 'Shared edit', 'tracks': {'OTIO_SCHEMA': 'Stack.1', 'children': tracks}}))


def setup(r):
    if not (MODELS / 'ggml-small.bin').exists():
        raise RuntimeError('run scripts/fixtures.sh first, missing the speech models')
    link_models(r)
    folder = r.work / 'otio'
    recording, pieces = record(folder)
    r.reel, r.shared, r.short = folder / 'Talking reel.otio', folder / 'shared.otio', folder / 'Short reel.otio'
    timeline(r.reel, recording, pieces)
    # The same recording cut shorter, so learning from it changes the style.
    timeline(r.short, recording, pieces[:4], 'Short reel')
    missing(r.shared, folder)
    r.cuts = len(pieces)


def click(r, text, scope=DIALOG):
    return r.s.run(CLICK_TEXT, text, scope)


def job(r):
    return r.s.run(JOB)


def shot(r, name):
    """A screenshot once the dialog has faded in or changed."""
    time.sleep(0.5)
    r.shot(name)


def learn(r, watch=True, ended=True):
    """Learns from the projects chosen; `watch` checks the job while it runs, which words recognised before skip."""
    before = job(r)
    click(r, 'Learn')
    if watch:
        bar = f"return document.querySelector('{DIALOG} [role=progressbar]')?.getAttribute('aria-label') ?? null"
        r.check('learning runs as a job, its row showing the phase over a progress bar',
                wait(lambda: (j := job(r)) and j['status'] == 'running' and j['phase'] and j['phase'] in r.s.run(DIALOG_TEXT)
                     and r.s.run(bar) == 'Talking reel', 20), job(r))
    if ended:
        return wait(lambda: (j := job(r)) and j != before and j['status'] != 'running', 600)


@flow('style-import', 'Learn my style from an .otio project cut in another editor: what it reads and misses, the job with Stop, '
                      'EDIT.md as `nuzky style learn --from` writes it, and replacing a style only after agreeing', before=setup, home=True)
def style_import(r):
    r.s.run("document.querySelector('nav[aria-label=Collections] button[data-row]:nth-of-type(2)').click()")
    r.check('Your style opens from the sidebar', wait(lambda: r.s.run(f"return !!document.querySelector('{PAGE}')"), 10))
    r.s.run(CLICK_TEXT, 'Learn my style', PAGE)
    r.check('Learn my style opens its dialog', wait(lambda: r.s.run(f"return !!document.querySelector('{DIALOG}')"), 5))
    # The keyboard moves between the tabs.
    r.s.run(f"document.querySelector('{DIALOG} [role=tab][aria-selected=true]').focus()")
    r.s.run(f"document.querySelector('{DIALOG} [role=tablist]').dispatchEvent(new KeyboardEvent('keydown', {{key: 'ArrowRight', bubbles: true}}))")
    tab = "return document.querySelector('[role=dialog] [role=tab][aria-selected=true]')?.textContent"
    r.check('the arrow key opens Projects (.otio)', wait(lambda: r.s.run(tab) == 'Projects (.otio)', 5), r.s.run(tab))
    r.check('and the tab keeps the focus', r.s.run("return document.activeElement.getAttribute('role')") == 'tab')
    r.check('which says what to export', 'OpenTimelineIO (.otio), which DaVinci Resolve' in r.s.run(DIALOG_TEXT), r.s.run(DIALOG_TEXT))
    shot(r, 'projects-empty')

    # Choosing files: the native dialog cannot be driven, so the flow hands its paths to what it calls.
    r.s.call('window.__nuzky.addProjects(arguments[0])', [str(r.reel), str(r.shared)])
    r.check('both projects are read at once', wait(lambda: len(r.s.run(PROJECTS) or []) == 2, 10), r.s.run(PROJECTS))
    reel, shared = r.s.run(PROJECTS)
    r.check('the reel plays its recording in its pieces', [(x['name'], x['clips'], x['missing']) for x in reel['recordings']] == [('talk.mp4', r.cuts, None)], reel)
    r.check('the shared project misses its recording', [(x['name'], x['missing']) for x in shared['recordings']] == [('their-talk.mp4', 'not on this computer')], shared)
    r.check('and never fetches its media from the web', shared['unread'] == ['Media that is not a file on this computer: https://example.com/broll.mp4'], shared['unread'])
    shown = r.s.run(DIALOG_TEXT)
    r.check('the dialog says what each will teach and what is missing',
            f'talk.mp4 · {r.cuts} clips' in shown and 'their-talk.mp4: not on this computer' in shown and 'Not read: Media that is not a file' in shown, shown)
    shot(r, 'projects-read')

    learn(r, ended=False)
    shot(r, 'projects-learning')
    started = time.monotonic()
    # From the keyboard: Stop has the focus when it goes away.
    r.s.run(f"[...document.querySelectorAll('{DIALOG} button')].find((b) => b.textContent === 'Stop').focus()")
    click(r, 'Stop')
    r.check('Stop ends learning within seconds', wait(lambda: (job(r) or {}).get('status') == 'cancelled', 10), job(r))
    r.check('nothing was learned or written', time.monotonic() - started < 10 and edit_md(r) is None and not view(r)['sources'])
    r.check('the focus stays in the dialog, on Learn', wait(lambda: r.s.run(
        f"return document.activeElement.closest('{DIALOG}') && document.activeElement.textContent"), 5) == 'Learn')
    r.check('the projects can be changed again', r.s.run(f"""return [...document.querySelectorAll('{DIALOG} button')].some((b) => b.textContent.trim() === 'Add projects')
        && !!document.querySelector('{DIALOG} button[aria-label="Remove Shared edit"]')"""))

    r.check('learning ends', learn(r), job(r))
    result = job(r)
    first, second = result['results']
    r.check('the reel teaches its recording', result['status'] == 'done' and first['learned'] == ['talk.mp4'] and not first['error'], result)
    r.check('the shared project says why it teaches nothing', second['learned'] == [] and 'their-talk.mp4' in second['skipped'][0][0], second)
    preview = wait(lambda: r.s.run(PREVIEW), 5)
    r.check('the dialog shows EDIT.md as learned', preview and preview.startswith('# Editing style') and '| talk.mp4 |' in preview, (preview or '')[:300])
    r.check('learning alone changes nothing, not even the suggestions', edit_md(r) is None and not view(r)['sources'] and not view(r)['suggestions'], view(r))
    cli = r.work / 'cli.md'
    subprocess.run([str(CLI), 'style', 'learn', '--from', str(r.reel), '--from', str(r.shared), '--out', str(cli)], env=r.env, check=True, capture_output=True)
    r.check('it is what nuzky style learn --from writes from the same files', preview == cli.read_text(), {'app': len(preview or ''), 'cli': len(cli.read_text())})
    shot(r, 'projects-learned')
    if r.check('the window goes to its smallest size', resize(r, 1024, 640)):
        shot(r, 'projects-learned-1024')
        box = r.s.run(f"""const d = document.querySelector('{DIALOG}').getBoundingClientRect();
            const b = [...document.querySelectorAll('{DIALOG} button')].find((x) => x.textContent === 'Use as my style').getBoundingClientRect();
            return [d.top >= 0 && d.bottom <= innerHeight, b.bottom <= d.bottom]""")
        r.check('the dialog and its buttons fit the smallest window', box == [True, True], box)
        resize(r, 1440, 900)

    # While an AI edits, nothing changes the style it follows.
    r.s.run("window.__nuzky.store.setState({aiRun: 'e2e-run'})")
    use = f"return [...document.querySelectorAll('{DIALOG} button')].find((b) => b.textContent === 'Use as my style')"
    locked = r.s.run(use + "?.getAttribute('aria-disabled')")
    click(r, 'Use as my style')
    time.sleep(1)
    r.check('Use as my style waits while the AI edits, and says why', locked == 'true' and r.s.run(use + '?.title') == AI_EDITING and edit_md(r) is None)
    r.s.run("window.__nuzky.store.setState({aiRun: null})")

    click(r, 'Use as my style')
    r.check('Use as my style writes EDIT.md as shown', wait(lambda: edit_md(r) == preview, 10), (edit_md(r) or '')[:200])
    r.check('and closes the dialog on the style', wait(lambda: not r.s.run(f"return !!document.querySelector('{DIALOG}')"), 5) and len(view(r)['rules']) >= 3)
    r.check('the style says where it was learned', view(r)['versions'][0]['label'] == 'Learned from 1 project'
            and any(s['kind'] == 'timeline' and s['title'] == 'talk.mp4 in Talking reel' for s in view(r)['sources']), view(r)['sources'])
    shot(r, 'used')

    rule = 'Never cut the sentence where I name the product.'
    r.s.run(TYPE, f'{PAGE} input[aria-label="Tell the AI what to do or not to do"]', rule)
    r.s.run("document.activeElement.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true}))")
    r.check('the creator adds a rule of their own', wait(lambda: f'- {rule}' in (edit_md(r) or ''), 10))
    mine = edit_md(r)

    r.s.run(CLICK_TEXT, 'Learn my style', PAGE)
    wait(lambda: r.s.run(f"return !!document.querySelector('{DIALOG}')"), 5)
    r.s.call('window.__nuzky.addProjects(arguments[0])', [str(r.short)])
    wait(lambda: len(r.s.run(PROJECTS) or []) == 1, 10)
    r.check('learning from a shorter cut ends', learn(r, watch=False) and job(r)['status'] == 'done', job(r))
    preview = wait(lambda: r.s.run(PREVIEW), 5)
    r.check('over a style it offers to replace it', wait(lambda: 'Replace my style' in r.s.run(DIALOG_TEXT), 5), r.s.run(DIALOG_TEXT)[-300:])
    click(r, 'Replace my style')
    shown = r.s.run(DIALOG_TEXT)
    r.check('and asks first, saying what gives way and what stays', 'Its learned rules, also those you changed by hand, give way; Your rules stay' in shown and edit_md(r) == mine, shown[-300:])
    r.check('with the focus on Cancel', r.s.run('return document.activeElement.textContent') == 'Cancel')
    shot(r, 'replace-confirm')
    r.s.run("document.activeElement.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true, cancelable: true}))")
    r.check('Esc answers the question and keeps the dialog', wait(lambda: 'Replace my style' in r.s.run(DIALOG_TEXT), 5) and edit_md(r) == mine)
    click(r, 'Replace my style')
    click(r, 'Cancel')
    r.check('Cancel leaves the style as it was', 'Replace my style' in r.s.run(DIALOG_TEXT) and edit_md(r) == mine)
    click(r, 'Replace my style')
    click(r, 'Replace')
    r.check('Replace writes the learned style as shown', wait(lambda: edit_md(r) == preview, 10) and preview != mine, (edit_md(r) or '')[:400])
    r.check('with the creator\'s own rule kept', f'- {rule}' in edit_md(r) and '| Short reel |' in edit_md(r), edit_md(r)[:400])
    undo = "return window.__nuzky.store.getState().toasts.find((t) => t.text === 'Saved as your style')?.action?.label ?? null"
    r.check('a toast offers Undo', r.s.run(undo) == 'Undo')
    r.s.run("window.__nuzky.store.getState().toasts.findLast((t) => t.text === 'Saved as your style').action.run()")
    r.check('Undo brings the style before it back', wait(lambda: edit_md(r) == mine, 10))
    r.check('no error toast', not r.errors(), r.errors())
