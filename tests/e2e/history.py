"""Versions: three AI runs and an edit by hand are listed in the top bar, Restore brings back the project as the
first run left it, Undo takes that back, and after a restart every version is still there."""
import json, time

from e2e.agent import agent_project
from e2e.harness import Bridge, Session, close_window, flow, wait
from e2e.home import resize

BUTTON = "document.querySelector('header button[aria-label^=\"Versions\"]')"
ROWS = "[...document.querySelectorAll('[role=menu][aria-label=Versions] [role=menuitemradio]')]"


def menu(r):
    """Opens the versions menu; its rows as label, time and whether it is the current version."""
    r.s.run(f'{BUTTON}.click()')
    wait(lambda: r.s.run(f'return {ROWS}.length > 0'), 5)
    time.sleep(0.5)  # painted before a screenshot
    return r.s.run(f"""return {ROWS}.map((row) => ({{label: row.querySelector('.flex-1').textContent,
        time: row.querySelector('.text-muted:last-child')?.textContent, current: row.getAttribute('aria-checked') === 'true'}}));""")


def choose(r, label):
    """Clicks the version. The menu closes when the window resizes, which a resize the flow made can still do
    late on a loaded machine, and opens slowly there; it is opened again until the row is there."""
    row = f"{ROWS}.find((row) => row.querySelector('.flex-1').textContent === arguments[0])"
    for _ in range(3):
        if wait(lambda: r.s.run(f'return !!{row}', label), 10):
            r.s.run(f'{row}.click()', label)
            return
        menu(r)
    raise AssertionError(f'{label} is not in the versions menu')


def alive(r):
    try:
        return r.s.run('return 1') == 1
    except Exception:
        return False


def close_menu(r):
    r.s.run("document.querySelector('[role=menu][aria-label=Versions]')?.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}))")


@flow('history', 'Three AI runs and an edit by hand are versions; Restore brings back the first run, Undo takes it back, and they outlive a restart',
      before=agent_project)
def history(r):
    after = {}
    bridge = Bridge(r, r.saved_project())
    try:
        for label, at in (('Split at 2 s', 2_000_000), ('Split at 4 s', 4_000_000), ('Split at 6 s', 6_000_000)):
            run = bridge.call('begin_run', {'label': label})
            if label == 'Split at 4 s':
                r.check('the UI knows an agent is editing', wait(lambda: r.state()['aiRun'], 10))
                locked = r.s.run(f"const b = {BUTTON}; return b && {{disabled: b.getAttribute('aria-disabled'), reason: b.title}};")
                r.check('Versions is locked while the AI edits, and says why',
                        locked and locked['disabled'] == 'true' and 'AI is done' in locked['reason'], locked)
            target = next(c for c in r.track() if c['startUs'] < at < c['startUs'] + c['durationUs'])
            bridge.call('apply_edits', {'run_id': run['run_id'], 'request_id': 'split', 'edits': [{'type': 'splitClip', 'clipId': target['id'], 'atUs': at}]})
            bridge.call('end_run', {'run_id': run['run_id'], 'action': 'keep'})
            r.check(f'the run "{label}" lands', wait(lambda: not r.state()['aiRun'] and len(r.track()) == len(after) + 2, 10), r.track())
            after[label] = r.track()
    finally:
        bridge.close()

    r.s.run('document.activeElement?.blur()')
    r.focus_clip(r.track()[0]['id'])
    r.key('Delete')
    edited = wait(lambda: (c := r.track()) and len(c) == 3 and c, 10)
    r.check('Delete by hand removes the first piece', edited, r.track())

    rows = menu(r)
    r.shot('versions')
    labels = [row['label'] for row in rows]
    r.check('the menu lists the edit by hand, the three runs and the opened project, newest first',
            labels == ['Delete', 'Split at 6 s', 'Split at 4 s', 'Split at 2 s', 'Opened'], rows)
    r.check('the newest one is current, with its time', rows[0]['current'] and not any(row['current'] for row in rows[1:]) and rows[0]['time'], rows)
    choose(r, 'Split at 2 s')
    r.check('Restore brings back the project exactly as the first run left it', wait(lambda: r.track() == after['Split at 2 s'], 10),
            {'now': r.track(), 'then': after['Split at 2 s']})
    toast = "return window.__nuzky.store.getState().toasts.find((t) => t.text === arguments[0])?.action?.label ?? null"
    r.check('a toast says what came back and offers Undo', wait(lambda: r.s.run(toast, 'Restored “Split at 2 s”') == 'Undo', 5), r.state()['toasts'])
    r.shot('restored')
    saved = lambda: len(json.loads(r.saved_project().read_text())['tracks'][0]['clips'])
    r.check('the restored project is saved', wait(lambda: saved() == 2, 10), saved())

    r.s.run('document.activeElement?.blur()')
    r.key('z', ctrlKey=True)
    r.check('Ctrl+Z takes the restore back', wait(lambda: r.track() == edited, 10), r.track())
    r.check('and that is saved too', wait(lambda: saved() == 3, 10), saved())

    close_window()
    r.check('the window closes', wait(lambda: not alive(r), 15))
    r.s.close()
    r.s = Session()
    r.check('the app starts again with the project',
            wait(lambda: r.s.run('return !!window.__nuzky?.store.getState().snap', retries=3), 60) and wait(lambda: r.track() == edited, 10), r.track())
    r.s.run("window.__nuzky.store.setState({view: 'editor'})")
    rows = menu(r)
    r.shot('versions-after-restart')
    labels = [row['label'] for row in rows]
    r.check('after the restart every version is still listed, with the restore and its undo',
            labels == ['Undo', 'Restore: Split at 2 s', 'Delete', 'Split at 6 s', 'Split at 4 s', 'Split at 2 s', 'Opened'], rows)
    r.check('the current one is the undo', rows[0]['current'], rows)
    close_menu(r)
    if r.check('the window goes to its smallest size', resize(r, 1024, 640)):
        menu(r)
        r.shot('versions-1024')
        box = r.s.run("const b = document.querySelector('[role=menu][aria-label=Versions]').getBoundingClientRect(); return [b.left, b.bottom, window.innerHeight]")
        r.check('the menu fits the smallest window', box[0] >= 0 and box[1] <= box[2], box)
        close_menu(r)
        resize(r, 1440, 900)
    menu(r)
    choose(r, 'Split at 4 s')
    r.check('a run from before the restart restores too', wait(lambda: r.track() == after['Split at 4 s'], 10), r.track())
    rows = menu(r)
    r.check('the current version is now the restore', rows[0] == {**rows[0], 'label': 'Restore: Split at 4 s', 'current': True}, rows[0])
    close_menu(r)
    r.check('no error toast', not r.errors(), r.errors())
