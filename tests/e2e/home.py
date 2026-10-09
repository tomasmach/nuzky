"""The home screen and the launcher. CapOpen starts on every project, with the last one open behind it: pictures
rendered like the export, collections, Rename, Duplicate, the Trash with Undo, and search through names and
through what was said. Over the editor, the launcher opens recent projects, starts new ones and searches the
same way. Opening another project while an agent edits asks whether to keep or undo its changes."""
import json, os, subprocess, time
from pathlib import Path

from e2e.harness import CLI, FIXTURES, Bridge, close_window, flow, link_models, wait, webdriver

DAY = 86_400
CARDS = """return [...document.querySelectorAll('[role=listbox][aria-label=Projects] [role=option][data-path]')].map((c) => ({
  path: c.dataset.path, name: c.querySelector('[data-name]')?.textContent ?? c.querySelector('input')?.value,
  meta: c.querySelector('[data-meta]').textContent, selected: c.getAttribute('aria-selected') === 'true',
  disabled: c.getAttribute('aria-disabled') === 'true'}));"""
SECTIONS = "return [...document.querySelectorAll('[role=listbox][aria-label=Projects] section > h2')].map((h) => h.textContent);"
# A poster that shows the test pattern has many colours; an empty or black frame has a few.
POSTER_COLOURS = """const img = document.querySelector(`[data-path="${CSS.escape(arguments[0])}"] img`);
if (!img || !img.complete || !img.naturalWidth) return 0;
const c = document.createElement('canvas'); c.width = img.naturalWidth; c.height = img.naturalHeight;
const g = c.getContext('2d'); g.drawImage(img, 0, 0);
const d = g.getImageData(0, 0, c.width, c.height).data; const seen = new Set();
for (let i = 0; i < d.length; i += 16) seen.add((d[i] >> 4) << 8 | (d[i + 1] >> 4) << 4 | d[i + 2] >> 4);
return seen.size;"""
TYPE = """const el = document.querySelector(arguments[0]); el.focus();
Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, arguments[1]);
el.dispatchEvent(new Event('input', {bubbles: true})); return true;"""
PRESS = """const el = arguments[0] ? document.querySelector(arguments[0]) : document.activeElement;
el.dispatchEvent(new KeyboardEvent('keydown', Object.assign({key: arguments[1], bubbles: true, cancelable: true}, arguments[2])));"""
CLICK_TEXT = """const scope = arguments[1] ? document.querySelector(arguments[1]) : document;
const el = [...scope.querySelectorAll('button, [role=menuitem], [role=menuitemradio], [role=option]')]
  .find((b) => b.textContent.trim().startsWith(arguments[0]));
if (!el) return false; el.click(); return true;"""
VIEW = "const s = window.__capopen.store.getState(); return {view: s.view, launcher: s.launcherOpen, name: s.snap.project.name, path: s.snap.path, timeUs: s.timeUs, tab: s.panelTab, aiRun: s.aiRun, canvas: [s.snap.project.canvas.width, s.snap.project.canvas.height]};"


def projects_dir(r):
    return r.work / 'data/capopen/projects'


def make(r, file, name, media, age_days):
    """A project of `media` named `name`, last changed `age_days` ago, as the CLI makes it."""
    path = projects_dir(r) / file
    subprocess.run([str(CLI), 'new', str(path), *map(str, media)], env=r.env, check=True, capture_output=True)
    project = json.loads(path.read_text())
    project['name'] = name
    path.write_text(json.dumps(project))
    stamp = time.time() - age_days * DAY
    os.utime(path, (stamp, stamp))
    return path


def seed(r):
    link_models(r)
    make(r, 'talk.capopen', 'Talk to camera', [FIXTURES / 'talk.mp4'], 0)
    make(r, 'croatia.capopen', 'Croatia day 2', [FIXTURES / 'wide.mp4'], 3)
    make(r, 'reel.capopen', 'Morning reel', [FIXTURES / 'portrait.mp4'], 40)
    broken = projects_dir(r) / 'broken.capopen'
    broken.write_text('{"name": "half a proj')
    empty = projects_dir(r) / 'empty.capopen'
    empty.write_text(json.dumps({'version': 1, 'name': 'Untitled project', 'canvas': {'width': 1080, 'height': 1920, 'fps': 30, 'background': '#000000'},
                                 'assets': [], 'tracks': [{'id': 'main', 'kind': 'video', 'name': 'Main', 'muted': False, 'hidden': False, 'clips': []}]}))
    for path, days in ((broken, 60), (empty, 20)):
        os.utime(path, (time.time() - days * DAY,) * 2)


def cards(r):
    return r.s.run(CARDS, retries=3) or []


def card(r, name):
    return next((c for c in cards(r) if c['name'] == name), None)


def view(r):
    return r.s.run(VIEW, retries=3)


def click(r, text, scope=None):
    return r.s.run(CLICK_TEXT, text, scope)


def press(r, key, selector=None, **modifiers):
    r.s.run(PRESS, selector, key, modifiers)


def focus_card(r, name):
    r.s.run("document.querySelector(`[data-path=\"${CSS.escape(arguments[0])}\"]`).focus()", card(r, name)['path'])


def toast_texts(r):
    return [t['text'] for t in r.state()['toasts']]


def resize(r, width, height):
    """The window at another size; False when the window manager does not allow it."""
    try:
        webdriver('POST', r.s.path + '/window/rect', {'width': width, 'height': height})
    except RuntimeError:
        return False
    return wait(lambda: r.s.run('return [window.innerWidth, window.innerHeight]') == [width, height], 5)


def wait_library(r):
    return wait(lambda: len(cards(r)) > 0, 20)


@flow('home', 'CapOpen starts on the home screen: every project with its picture, collections, rename, duplicate, the '
      'Trash with Undo and search through names and through what was said', before=seed, home=True)
def home(r):
    r.check('CapOpen starts on the home screen, the newest project open behind it',
            wait(lambda: view(r)['view'] == 'home', 10) and view(r)['name'] == 'Talk to camera', view(r))
    r.check('the back button names the open project',
            r.s.run("return [...document.querySelectorAll('header button')].some((b) => b.textContent.includes('Talk to camera'))"))
    wait_library(r)
    names = sorted(c['name'] for c in cards(r))
    r.check('every project shows, the damaged one by its file name', names == ['Croatia day 2', 'Morning reel', 'Talk to camera', 'broken.capopen'], names)
    r.check('an empty untitled project nobody has open is cleared away', not (projects_dir(r) / 'empty.capopen').exists())
    r.check('the damaged project says why and cannot open', (card(r, 'broken.capopen') or {}).get('meta') == 'Damaged file'
            and card(r, 'broken.capopen')['disabled'], card(r, 'broken.capopen'))
    r.check('the open project says so', (card(r, 'Talk to camera') or {}).get('meta') == 'Open now', card(r, 'Talk to camera'))
    sections = r.s.run(SECTIONS)
    order = [c['name'] for c in cards(r)]
    r.check('a few projects show newest first, without date groups', sections == [] and order[0] == 'Talk to camera'
            and order.index('Croatia day 2') < order.index('Morning reel'), (sections, order))
    for name in ('Talk to camera', 'Croatia day 2', 'Morning reel'):
        path = card(r, name)['path']
        colours = wait(lambda: r.s.run(POSTER_COLOURS, path) > 40 and r.s.run(POSTER_COLOURS, path), 40)
        r.check(f'{name} shows its picture, rendered like the export', colours, colours)
    r.shot('home')
    if resize(r, 1024, 640):
        time.sleep(0.5)
        r.shot('home-1024')
        resize(r, 1440, 900)
    else:
        r.check('the window can be resized to its smallest size for a screenshot', False)

    # Collections: a new one, a project moved into it from its menu.
    click(r, 'New collection', 'nav[aria-label=Collections]')
    r.s.run(TYPE, "input[aria-label='New collection name']", 'Client work')
    press(r, 'Enter', "input[aria-label='New collection name']")
    r.check('a new collection shows and opens, empty with a hint',
            wait(lambda: r.s.run("return document.querySelector('main h1')?.textContent") == 'Client work'
                 and 'No projects in Client work yet' in r.s.run('return document.body.textContent'), 5))
    r.shot('empty-collection')
    click(r, 'All projects', 'nav[aria-label=Collections]')
    wait(lambda: len(cards(r)) == 4, 5)
    r.s.run("""const c = document.querySelector(`[data-path="${CSS.escape(arguments[0])}"]`); const b = c.getBoundingClientRect();
        c.dispatchEvent(new MouseEvent('contextmenu', {bubbles: true, clientX: b.left + 60, clientY: b.top + 80}));""", card(r, 'Croatia day 2')['path'])
    r.check('right-click opens the project menu', wait(lambda: r.s.run("return !!document.querySelector('[role=menu]')"), 3))
    click(r, 'Move to collection', '[role=menu]')
    time.sleep(0.3)
    r.shot('context-menu')
    menus = r.s.run("return [...document.querySelectorAll('[role=menu]')].map((m) => m.getAttribute('aria-label'))")
    r.check('Move to collection opens the list of collections', menus == ['Croatia day 2', 'Move to collection'], menus)
    # The pointer moves on to Rename: the list closes and the keys work in the project menu again.
    r.s.run("[...document.querySelectorAll('[role=menu]')][0].querySelectorAll('[role=menuitem]').forEach((i) => "
            "i.textContent.startsWith('Rename') && i.dispatchEvent(new PointerEvent('pointerover', {bubbles: true})))")
    focus = lambda: r.s.run("const m = document.querySelectorAll('[role=menu]'); return [m.length, document.activeElement === m[0]]")
    r.check('moving the pointer off the list closes it and the project menu keeps the keyboard', wait(lambda: focus() == [1, True], 3), focus())
    click(r, 'Move to collection', '[role=menu]')
    wait(lambda: len(r.s.run("return [...document.querySelectorAll('[role=menu]')]")) == 2, 3)
    r.s.run("[...document.querySelectorAll('[role=menu]')][1].querySelectorAll('[role=menuitemradio]').forEach((i) => i.textContent.includes('Client work') && i.click())")
    croatia = card(r, 'Croatia day 2')['path']
    library = r.work / 'data/capopen/library.json'
    r.check('the project is in the collection, saved in library.json',
            wait(lambda: library.exists() and croatia in json.loads(library.read_text())['members'], 5)
            and wait(lambda: 'Client work' in card(r, 'Croatia day 2')['meta'], 5), card(r, 'Croatia day 2'))
    count = r.s.run("return [...document.querySelectorAll('nav[aria-label=Collections] [data-drop]')].map((b) => b.textContent)")
    r.check('the sidebar counts it', count == ['Client work1'], count)

    # Rename and duplicate a project that is not open, from the keyboard.
    focus_card(r, 'Croatia day 2')
    r.key('F2')
    r.check('F2 renames in place', wait(lambda: r.s.run("return document.activeElement?.getAttribute('aria-label')") == 'Project name', 3))
    r.s.run(TYPE, "input[aria-label='Project name']", 'Croatia day two')
    press(r, 'Enter', "input[aria-label='Project name']")
    r.check('the new name is saved in the project file',
            wait(lambda: json.loads((projects_dir(r) / 'croatia.capopen').read_text())['name'] == 'Croatia day two', 5)
            and wait(lambda: card(r, 'Croatia day two'), 5))
    focus_card(r, 'Croatia day two')
    r.key('d', ctrlKey=True)
    copy = wait(lambda: card(r, 'Croatia day two copy'), 5)
    r.check('Ctrl+D duplicates into the same collection and focuses the copy', copy and 'Client work' in copy['meta']
            and wait(lambda: r.s.run('return document.activeElement?.dataset.path') == copy['path'], 3), copy)

    # The Trash, with Undo; after a while the file really goes to the system Trash, the videos stay.
    r.s.run("document.querySelector(`[data-path=\"${CSS.escape(arguments[0])}\"]`).focus()", copy['path'])
    r.key('Delete')
    first_move = time.time()
    r.check('Delete moves the copy to the Trash, the toast says the videos stay and offers Undo',
            wait(lambda: not card(r, 'Croatia day two copy'), 5)
            and any('Moved “Croatia day two copy” to the Trash. Your videos stay where they are.' == t for t in toast_texts(r)), toast_texts(r))
    r.shot('trash-toast')
    r.s.run("[...document.querySelectorAll('[data-toast]')].find((t) => t.textContent.includes('to the Trash')).querySelector('button').click()")
    r.check('Undo brings it back', wait(lambda: card(r, 'Croatia day two copy'), 5))
    # Moved again 20 s later: the first move's timer must not take this one.
    time.sleep(20)
    r.s.run("document.querySelector(`[data-path=\"${CSS.escape(arguments[0])}\"]`).focus()", copy['path'])
    r.key('Delete')
    moved = time.time()
    # And a project an agent opens while it waits for the Trash stays, with a message.
    reel = card(r, 'Morning reel')['path']
    r.s.run("document.querySelector(`[data-path=\"${CSS.escape(arguments[0])}\"]`).focus()", reel)
    r.key('Delete')
    agent = Bridge(r, projects_dir(r) / 'reel.capopen')
    try:
        time.sleep(max(0, first_move + 33 - time.time()))
        r.check('30 s after the first move, the copy moved again is still waiting for Undo', os.path.exists(copy['path'])
                and not card(r, 'Croatia day two copy'))
        trashed = r.work / 'data/Trash/files'
        r.check('30 s after the second move the copy is in the system Trash and gone from the projects',
                wait(lambda: trashed.exists() and any(p.suffix == '.capopen' for p in trashed.iterdir()), max(5, moved + 45 - time.time()))
                and not os.path.exists(copy['path']), sorted(os.listdir(trashed)) if trashed.exists() else None)
        r.check('a project an agent opened meanwhile stays, and a toast says why',
                wait(lambda: any('“Morning reel” was opened in another CapOpen window or by an AI agent' in t for t in toast_texts(r)), 10)
                and os.path.exists(reel) and wait(lambda: card(r, 'Morning reel'), 5), toast_texts(r))
    finally:
        agent.close()
    r.check('the videos it used are untouched', (FIXTURES / 'wide.mp4').exists())
    open_card = card(r, 'Talk to camera')['path']
    r.s.run("document.querySelector(`[data-path=\"${CSS.escape(arguments[0])}\"]`).focus()", open_card)
    r.key('Delete')
    r.check('the open project cannot go to the Trash, a toast says why',
            wait(lambda: any('Open another project first' in t for t in toast_texts(r)), 3) and os.path.exists(open_card), toast_texts(r))

    # Search: names at once, then what was said once the project is transcribed.
    models = r.s.call('window.__capopen.api.speechModels().then((m) => JSON.stringify(m))')
    small = next(m['id'] for m in json.loads(models['value']) if 'small' in m['id'] and m['downloaded'])
    job = r.s.call("window.__capopen.api.startTranscript(arguments[0], 'en', false, window.__capopen.store.getState().snap.sessionEpoch)", small)
    r.check('the open project is transcribed', job['ok'] and wait(lambda: any(j['kind'] == 'transcript' and j['status'] == 'done' for j in r.state()['jobs']), 240),
            r.state()['jobs'])
    r.s.run(TYPE, 'main input[type=search]', 'croatia')
    names = wait(lambda: [c['name'] for c in cards(r)] == ['Croatia day two'] and [c['name'] for c in cards(r)], 3)
    r.check('typing a name shows only the projects that match', names, [c['name'] for c in cards(r)])
    r.s.run(TYPE, 'main input[type=search]', 'laptop')
    quote = """const s = document.querySelector('section[aria-label="Said in your videos"]');
        return s && [...s.querySelectorAll('button')].map((b) => ({text: b.textContent, mark: b.querySelector('mark')?.textContent}));"""
    hits = wait(lambda: (r.s.run(quote) or None), 20)
    r.check('a word said in a video finds the project, with the sentence around it', hits and 'Talk to camera' in hits[0]['text']
            and hits[0]['mark'].lower().startswith('laptop'), hits)
    r.check('projects that are not transcribed are named, so nothing is silently missed',
            "aren't transcribed" in (r.s.run("return document.querySelector('section[aria-label=\"Said in your videos\"]').textContent") or ''))
    r.shot('search-said')

    # In a collection, what was said in its projects is found even when ten newer projects elsewhere,
    # each saying "the" more than three times, fill the limit on results.
    talk = r.s.run("return window.__capopen.library.getState().projects.find((p) => p.name === 'Talk to camera').path")
    for i in range(10):
        make(r, f'again-{i}.capopen', f'Talk again {i}', [FIXTURES / 'talk.mp4'], -0.01)
    client = r.s.run("return window.__capopen.library.getState().collections.find((c) => c.name === 'Client work').id")
    r.s.call('window.__capopen.api.setCollection([arguments[0]], arguments[1]).then(() => window.__capopen.refreshLibrary())', talk, client)
    click(r, 'Client work', 'nav[aria-label=Collections]')
    r.s.run(TYPE, 'main input[type=search]', 'the')
    hits = wait(lambda: (r.s.run(quote) or None), 20)
    r.check('in a collection, a word said in its project is found although newer projects elsewhere say it more',
            hits and any('Talk to camera' in h['text'] for h in hits), hits)
    r.s.run(TYPE, 'main input[type=search]', 'laptop')
    wait(lambda: (r.s.run(quote) or [{}])[0].get('mark', '').lower().startswith('laptop'), 20)
    r.s.run("document.querySelector('section[aria-label=\"Said in your videos\"] button').click()")
    v = wait(lambda: view(r)['view'] == 'editor' and view(r), 5)
    r.check('a found sentence opens its project there, with the transcript', v and v['name'] == 'Talk to camera' and v['timeUs'] > 1_000_000
            and v['tab'] == 'transcript', v)
    unexpected = [e for e in r.errors() if 'so it was not moved to the Trash' not in e]
    r.check('no other error toast', not unexpected, unexpected)

    # Collections that cannot be read are never written over.
    index = r.work / 'data/capopen/library.json'
    before, mode = index.read_bytes(), index.stat().st_mode & 0o777
    os.chmod(index, 0)
    try:
        made = r.s.call("window.__capopen.api.createCollection('Lost')")
    finally:
        os.chmod(index, mode)
    r.check('a new collection is refused while the collections cannot be read, and they stay as they were',
            not made['ok'] and index.read_bytes() == before, made)

    # A list that was on its way when a project went to the Trash does not bring it back.
    names = "return Object.fromEntries(window.__capopen.library.getState().projects.map((p) => [p.name, p.path]))"
    croatia = r.s.run(names)['Croatia day two']
    r.s.call("""(async () => {
        const c = window.__capopen, real = c.api.library;
        let release;
        const gate = new Promise((ok) => (release = ok));
        c.api.library = () => real().then((list) => gate.then(() => list));
        const listed = c.refreshLibrary();
        await new Promise((ok) => setTimeout(ok, 500));
        c.api.library = real;
        await c.trashProjects([arguments[0]]);
        release();
        await listed;
    })()""", croatia)
    shown = r.s.run(names)
    r.check('a list on its way when a project goes to the Trash does not bring it back', 'Croatia day two' not in shown, sorted(shown))
    r.s.call('window.__capopen.api.restoreProjects([arguments[0]]).then(() => window.__capopen.refreshLibrary())', croatia)

    # Closing the window moves what waits for the Trash. A project an agent opened meanwhile stays,
    # and the window stays open to say why.
    r.s.run("window.__capopen.store.setState({view: 'home', toasts: []})")
    reel = r.s.run(names)['Morning reel']
    r.s.call('window.__capopen.trashProjects([arguments[0]])', reel)
    agent = Bridge(r, Path(reel))
    try:
        close_window()

        def told():
            try:
                return any('“Morning reel” was opened in another CapOpen window or by an AI agent' in t for t in toast_texts(r))
            except Exception:
                return False
        r.check('closing the window while an agent holds a project waiting for the Trash: the window stays open and says why',
                wait(told, 10) and os.path.exists(reel))
    finally:
        agent.close()


def launcher_seed(r):
    make(r, 'talk.capopen', 'Talk to camera', [FIXTURES / 'talk.mp4'], 0)
    make(r, 'reel.capopen', 'Morning reel', [FIXTURES / 'portrait.mp4'], 1)
    make(r, 'croatia.capopen', 'Croatia day 2', [FIXTURES / 'wide.mp4'], 2)


@flow('launcher', 'Over the editor, Projects or Ctrl+K opens the launcher: open, start or find a project. Switching '
      'while an agent edits asks whether to keep or undo its changes, and an agent holding a project shows it busy',
      before=launcher_seed)
def launcher(r):
    r.check('the flow starts in the editor of the newest project', view(r)['view'] == 'editor' and view(r)['name'] == 'Talk to camera', view(r))
    r.s.run('document.activeElement?.blur()')
    r.key('k', ctrlKey=True)
    r.check('Ctrl+K opens the launcher with the open project and the recent ones',
            wait(lambda: r.s.run("return [...document.querySelectorAll('#launcher-list [role=option]')].map((o) => o.textContent)"), 5))
    options = wait(lambda: len(r.s.run("return [...document.querySelectorAll('#launcher-list [role=option]')]")) == 4
                   and r.s.run("return [...document.querySelectorAll('#launcher-list [role=option]')].map((o) => o.textContent)"), 10)
    r.check('Continue, the two other projects and All projects', options and 'Talk to camera' in options[0] and 'Morning reel' in options[1]
            and 'All projects' in options[3], options)
    r.check('focus is in the search field', r.s.run("return document.activeElement?.getAttribute('role')") == 'combobox')
    time.sleep(0.3)
    r.shot('launcher')
    if resize(r, 1024, 640):
        time.sleep(0.5)
        r.shot('launcher-1024')
        resize(r, 1440, 900)
    r.s.run(TYPE, '[role=combobox]', 'croat')
    r.check('typing narrows to the project', wait(lambda: 'Croatia day 2' in (r.s.run("return document.querySelector('#launcher-list [role=option][aria-selected=true]')?.textContent") or ''), 5))
    press(r, 'Enter', '[role=combobox]')
    r.check('Enter opens it in the editor', wait(lambda: view(r)['name'] == 'Croatia day 2' and not view(r)['launcher'], 10), view(r))
    r.check('opening a project counts as opening it: it is first in the list next time',
            wait(lambda: r.s.call('window.__capopen.api.library().then((l) => l.projects[0].name)')['value'] == 'Croatia day 2', 5))

    # An agent edits the open project; opening another one asks what happens to its changes.
    croatia = r.work / 'data/capopen/projects/croatia.capopen'
    bridge = Bridge(r, croatia)
    try:
        # The user's own change, then the agent's. Undo changes while the new project cannot start
        # takes back only the agent's, and its toast offers no Undo that would take the user's.
        r.s.call("window.__capopen.store.getState().edit({type: 'renameProject', name: 'Croatia, day 2'})")
        wait(lambda: view(r)['name'] == 'Croatia, day 2', 5)
        run = bridge.call('begin_run', {'label': 'Split for a look'})
        wait(lambda: view(r)['aiRun'], 10)
        bridge.call('apply_edits', {'run_id': run['run_id'], 'request_id': 'look', 'edits': [{'type': 'splitClip', 'clipId': r.track()[0]['id'], 'atUs': 1_000_000}]})
        wait(lambda: len(r.track()) == 2, 10)
        r.s.call('window.__capopen.newProjectFromMedia(arguments[0])', [str(r.work / 'missing.mp4')])
        wait(lambda: r.s.run("return document.getElementById('switch-title')?.textContent"), 5)
        click(r, 'Undo changes and start')
        stayed = wait(lambda: any('None of those files could be opened' in e for e in r.errors()) and len(r.track()) == 1 and view(r), 10)
        r.check("Undo changes and start with videos that cannot open: the agent's edit is gone, the project and the user's change stay",
                stayed and stayed['name'] == 'Croatia, day 2', view(r))
        run_toasts = """return [...document.querySelectorAll('[data-toast]')].filter((t) => t.textContent.includes('Split for a look'))
            .map((t) => ({text: t.textContent, undo: [...t.querySelectorAll('button')].some((b) => b.textContent === 'Undo')}));"""
        ended = wait(lambda: r.s.run(run_toasts), 5)
        r.check('its toast says the AI edit was undone and offers no Undo, which would take back the change before it',
                ended and all('Undid the AI edit' in t['text'] and not t['undo'] for t in ended), ended)

        # Undo changes that cannot be saved: the run stays, and Stop and edit later still takes it back.
        run = bridge.call('begin_run', {'label': 'Split once more'})
        wait(lambda: view(r)['aiRun'], 10)
        bridge.call('apply_edits', {'run_id': run['run_id'], 'request_id': 'more', 'edits': [{'type': 'splitClip', 'clipId': r.track()[0]['id'], 'atUs': 3_000_000}]})
        wait(lambda: len(r.track()) == 2, 10)
        r.s.run("window.__capopen.store.setState({toasts: []})")
        r.s.call('window.__capopen.newProjectFromMedia(arguments[0])', [str(r.work / 'missing.mp4')])
        wait(lambda: r.s.run("return document.getElementById('switch-title')?.textContent"), 5)
        mode = projects_dir(r).stat().st_mode & 0o777
        os.chmod(projects_dir(r), 0o555)
        try:
            click(r, 'Undo changes and start')
            refused = wait(lambda: r.errors(), 10)
        finally:
            os.chmod(projects_dir(r), mode)
        r.check('Undo changes that cannot be saved says so and leaves the run open', refused and view(r)['aiRun'], refused)
        click(r, 'Stop and edit')
        more = """return [...document.querySelectorAll('[data-toast]')].filter((t) => t.textContent.includes('Split once more'))
            .map((t) => ({text: t.textContent, undo: [...t.querySelectorAll('button')].some((b) => b.textContent === 'Undo')}));"""
        ended = wait(lambda: r.s.run(more), 10)
        r.check('Stop and edit afterwards still takes the AI edit back, and offers no Undo of the change before it',
                ended and all('Undid the AI edit' in t['text'] and not t['undo'] for t in ended) and len(r.track()) == 1
                and view(r)['name'] == 'Croatia, day 2', ended)
        r.s.run("window.__capopen.store.setState({toasts: []})")

        run = bridge.call('begin_run', {'label': 'Trim the start'})
        r.check('the AI run shows', wait(lambda: view(r)['aiRun'], 10))
        clip = r.track()[0]
        bridge.call('apply_edits', {'run_id': run['run_id'], 'request_id': 'split', 'edits': [{'type': 'splitClip', 'clipId': clip['id'], 'atUs': 2_000_000}]})
        r.check("the agent's edit is on the timeline and saved", wait(lambda: len(r.track()) == 2, 10)
                and wait(lambda: len(json.loads(croatia.read_text())['tracks'][0]['clips']) == 2, 5))
        r.key('k', ctrlKey=True)
        wait(lambda: r.s.run("return !!document.querySelector('[role=combobox]')"), 5)
        r.s.run(TYPE, '[role=combobox]', 'morning')
        wait(lambda: 'Morning reel' in (r.s.run("return document.querySelector('#launcher-list [role=option][aria-selected=true]')?.textContent") or ''), 5)
        press(r, 'Enter', '[role=combobox]')
        question = wait(lambda: r.s.run("return document.getElementById('switch-title')?.textContent"), 5)
        r.check('opening another project asks what happens to the AI edit', question == 'Stop the AI edit and open “Morning reel”?', question)
        r.check('Keep changes and open is the default', r.s.run('return document.activeElement?.textContent') == 'Keep changes and open')
        time.sleep(0.3)
        r.shot('switch-confirm')
        click(r, 'Undo changes and open')
        r.check('Undo changes and open takes the AI edit back, saved, and opens the other project',
                wait(lambda: view(r)['name'] == 'Morning reel', 10)
                and wait(lambda: len(json.loads(croatia.read_text())['tracks'][0]['clips']) == 1, 5), view(r))
    finally:
        bridge.close()

    # An agent holding a project that is not open here: the home screen shows it busy and does not open it.
    talk = r.work / 'data/capopen/projects/talk.capopen'
    holder = Bridge(r, talk)
    try:
        holder.call('get_state', {})
        r.key('k', ctrlKey=True)
        wait(lambda: r.s.run("return !!document.querySelector('[role=combobox]')"), 5)
        click(r, 'All projects', '#launcher-list')
        r.check('All projects goes to the home screen', wait(lambda: view(r)['view'] == 'home', 5))
        busy = wait(lambda: (card(r, 'Talk to camera') or {}).get('meta') == 'In use elsewhere' and card(r, 'Talk to camera'), 10)
        r.check('a project an agent has open shows In use elsewhere and cannot open', busy and busy['disabled'], card(r, 'Talk to camera'))
        r.shot('home-busy')
    finally:
        holder.close()

    # New project in a format, and from videos in the first video's format.
    count = len(list(projects_dir(r).glob('*.capopen')))
    r.s.run("document.querySelector('[data-new-project]').click()")
    wait(lambda: r.s.run("return !!document.querySelector('[role=menu]')"), 3)
    time.sleep(0.2)
    r.shot('new-project-menu')
    click(r, '16:9', '[role=menu]')
    v = wait(lambda: view(r)['view'] == 'editor' and view(r)['name'] == 'Untitled project' and view(r), 10)
    r.check('New project, 16:9, opens an empty 16:9 project', v and v['canvas'] == [1920, 1080], v)
    r.check('it is a new file', len(list(projects_dir(r).glob('*.capopen'))) == count + 1)
    r.s.call('window.__capopen.newProjectFromMedia(arguments[0])', [str(FIXTURES / 'portrait.mp4')])
    v = wait(lambda: view(r)['canvas'] == [1080, 1920] and len(r.track()) == 1 and view(r), 15)
    r.check('a new project from a portrait video is 9:16 with the video on the timeline', v, view(r))
    r.check('the empty project left behind is cleared away once the list is shown again',
            r.s.call('window.__capopen.api.library().then((l) => l.projects.filter((p) => p.state === "empty").length)')['value'] == 0
            and len(list(projects_dir(r).glob('*.capopen'))) == count + 1)

    # Asked one after the other while the first is still reading its videos: the one asked last stays open.
    reel = str(projects_dir(r) / 'reel.capopen')
    r.s.run("window.__capopen.newProjectFromMedia(arguments[0]); window.__capopen.openProject({path: arguments[1], name: 'Morning reel'});",
            [str(FIXTURES / name) for name in ('talk.mp4', 'wide.mp4', 'portrait.mp4') * 3], reel)
    opened = wait(lambda: view(r)['name'] == 'Morning reel', 15)
    time.sleep(3)
    r.check('a new project from videos, then Morning reel at once: Morning reel is what stays open',
            opened and view(r)['name'] == 'Morning reel', view(r))
    unexpected = [e for e in r.errors() if 'None of those files could be opened' not in e]
    r.check('no other error toast', not unexpected, unexpected)


@flow('first-run', 'The first start offers the formats to begin with; choosing one shapes the project CapOpen made '
      'instead of leaving a second empty file', home=True)
def first_run(r):
    r.check('the first start shows Start your first project',
            wait(lambda: 'Start your first project' in r.s.run('return document.body.textContent'), 10))
    r.shot('first-run')
    click(r, '16:9', 'main')
    v = wait(lambda: view(r)['view'] == 'editor' and view(r), 5)
    r.check('a format opens the editor with that format', v and v['canvas'] == [1920, 1080], v)
    r.check('no second project file', wait(lambda: len(list(projects_dir(r).glob('*.capopen'))) == 1, 3),
            sorted(p.name for p in projects_dir(r).iterdir()))
