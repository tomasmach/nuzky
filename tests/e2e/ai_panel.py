import json, os, subprocess, time
from pathlib import Path

from e2e.harness import CLI, FIXTURES, flow, wait, webdriver

FAKE = Path(__file__).resolve().parent / 'fake_agent'
PANEL = """const p = document.querySelector('aside[aria-label=AI]'); if (!p) return null; const b = p.getBoundingClientRect();
return {left: Math.round(b.left), top: Math.round(b.top), width: Math.round(b.width), right: Math.round(window.innerWidth - b.right),
        bottom: Math.round(window.innerHeight - b.bottom), text: p.innerText, slot: p.dataset.dockSlot ?? null,
        inspector: !!document.querySelector('aside[aria-label=Inspector]'), windowWidth: window.innerWidth};"""
CLICK = """const scope = arguments[1] ? document.querySelector(arguments[1]) : document;
const all = [...scope.querySelectorAll('button, [role^=menuitem]')];
const b = all.find((b) => b.textContent.trim() === arguments[0] || b.getAttribute('aria-label') === arguments[0])
  ?? all.find((b) => b.textContent.trim().startsWith(arguments[0]));
if (!b) return null; b.click(); return b.getAttribute('aria-disabled') ?? 'false';"""
DRAG = """const g = document.querySelector('aside[aria-label=AI] button[aria-label="Move panel"]').getBoundingClientRect();
const at = (type, x, y) => new PointerEvent(type, {bubbles: true, button: 0, clientX: x, clientY: y, pointerId: 1});
document.querySelector('aside[aria-label=AI] button[aria-label="Move panel"]').dispatchEvent(at('pointerdown', g.left + 6, g.top + 6));
window.dispatchEvent(at('pointermove', arguments[0], arguments[1]));
if (arguments[2]) window.dispatchEvent(at('pointerup', arguments[0], arguments[1]));"""


def setup(r):
    project = r.work / 'data/nuzky/projects/panel.nuzky'
    subprocess.run([str(CLI), 'new', str(project), str(FIXTURES / 'talk.mp4')], env=r.env, check=True, capture_output=True)
    # The fake stands in for Claude Code; Codex is deliberately missing, whatever this machine has.
    r.env.update(NUZKY_AGENT_CLAUDE=str(FAKE), NUZKY_AGENT_CODEX=str(r.work / 'no-codex'), NUZKY_FAKE_AGENT_LOG=str(r.work / 'fake'))


def runs(r):
    log = r.work / 'fake/runs.jsonl'
    return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []


def panel(r):
    return r.s.run(PANEL, retries=3)


def agent(r):
    return r.s.run("const a = window.__nuzky.agent.getState(); return {status: a.status, draft: a.draft, kinds: a.items.map((i) => i.kind)};")


def click(r, label, scope='aside[aria-label=AI]'):
    return r.s.run(CLICK, label, scope)


def send(r, text):
    """Fills the field as typing does (WebKitWebDriver cannot send keys to it), then presses Enter."""
    r.s.run("""const t = document.querySelector('aside[aria-label=AI] textarea'); t.focus();
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(t, arguments[0]);
        t.dispatchEvent(new Event('input', {bubbles: true}));""", text)
    r.key('Enter')


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def idle(r):
    return wait(lambda: agent(r)['status'] == 'idle', 20)


@flow('ai_panel', "The AI panel runs the user's agent with only Nuzky's tools: it edits live, says exactly what changed, "
      'offers choices, stops at once, keeps a failed message, and docks, floats and resizes where the user put it',
      before=setup)
def ai_panel(r):
    r.check('the panel starts closed, with AI in the top bar', panel(r) is None and r.s.run(
        "return !![...document.querySelectorAll('header button')].find((b) => b.textContent.trim() === 'AI')"))
    r.s.run('document.body.focus()')
    r.key('j', ctrlKey=True)
    p = wait(lambda: panel(r), 5)
    r.check('Ctrl+J opens it as a column at the right edge, under the top bar to the bottom, beside the inspector',
            p and p['right'] == 6 and p['top'] == 48 and p['bottom'] == 6 and p['width'] == 360 and p['inspector'], p)
    r.check('the field takes focus', wait(lambda: r.s.run('return document.activeElement?.tagName') == 'TEXTAREA', 3))
    agents = wait(lambda: r.s.run('return window.__nuzky.agent.getState().agents'), 10) or []
    r.check('Claude Code is found and Codex is not installed', [(a['name'], bool(a['path'])) for a in agents] == [('Claude Code', True), ('Codex', False)], agents)
    click(r, 'Add captions')
    r.check('a starting point goes into the field to send or change, not straight to the agent',
            wait(lambda: agent(r)['draft'] == 'Add captions', 3) and not runs(r), agent(r))
    r.s.run("window.__nuzky.agent.setState({draft: ''})")
    clip = r.track()[0]
    r.s.run('window.__nuzky.store.getState().select([arguments[0]])', clip['id'])
    r.check('the selection shows as a chip that goes with the message', wait(lambda: '1 clip' in panel(r)['text'], 5), panel(r)['text'])
    r.shot('panel-open')

    # A cut with captions: live, explained by what changed, undone in one step.
    before = r.track()
    send(r, 'Cut the pause at the start and add captions')
    r.check('the editor locks while the agent edits', wait(lambda: r.state()['aiRun'], 15))
    r.check('its steps show live with plain titles', wait(lambda: 'Read the project' in panel(r)['text'], 10), panel(r)['text'])
    r.shot('working')
    wait(lambda: agent(r)['status'] == 'idle' and 'run' in agent(r)['kinds'], 30)
    text = panel(r)['text']
    r.check('a card lists exactly what changed, from the project itself',
            'Shortened the video from' in text and 'Cut out 1 passage, 2.0 s in total' in text and 'Added 2 captions' in text, text)
    r.check("the agent's answer streams in", 'Hotovo.' in text, text)
    r.s.run("[...document.querySelectorAll('aside[aria-label=AI] button')].find((b) => /^\\d+ steps$/.test(b.textContent.trim())).click()")
    r.check('unfolded steps name the scan and the frames it checked',
            wait(lambda: 'Scanning picture and sound' in panel(r)['text'] and 'Checking frames' in panel(r)['text'] and '00:00 · 00:01' in panel(r)['text'], 5), panel(r)['text'])
    r.check('the card takes the place of the "AI edit done" toast', not any('AI edit done' in t['text'] for t in r.state()['toasts']), r.state()['toasts'])
    saved = json.loads(r.saved_project().read_text())
    captions = [t for t in saved['tracks'] if t['name'] == 'Captions']
    r.check('the cut and the captions are in the saved project', captions and len(captions[0]['clips']) == 2, [t['name'] for t in saved['tracks']])
    first = runs(r)[0]
    r.check("the agent ran with Nuzky's tools only: built-ins and hooks off, in a folder outside home",
            not first['missing'] and not first['cwd'].startswith(os.environ['HOME']) and not first['resume'], first)
    r.check('the message carried the selection and the playhead', f"Selected clips: {clip['id']}" in first['prompt'] and '[Nuzky] Playhead' in first['prompt'], first['prompt'])
    r.shot('run-done')
    click(r, 'Added 2 captions')
    r.check('a line of the card selects the clips it is about', wait(lambda: len(r.s.run('return window.__nuzky.store.getState().selection')) == 2, 5))
    click(r, 'Undo')
    r.check('Undo on the card takes back the whole run', wait(lambda: len(r.track()) == len(before) and not any(t['name'] == 'Captions' for t in r.state()['tracks']), 10), r.state()['tracks'])
    undo = "return [...document.querySelectorAll('aside[aria-label=AI] button')].some((b) => b.textContent.trim() === 'Undo')"
    r.check('then the card says the run was undone and offers no Undo', wait(lambda: 'Undone: Tighten the start' in panel(r)['text'] and not r.s.run(undo), 5), panel(r)['text'])

    # Choices: the next message continues the conversation; a picked option is the next message.
    send(r, 'What options do I have?')
    r.check('the next message continues the same conversation', wait(lambda: len(runs(r)) == 2, 10) and runs(r)[1]['resume'] and runs(r)[1]['session'] == first['session'], runs(r)[-1])
    buttons = "return [...document.querySelectorAll('aside[aria-label=AI] [role=group] button')].map((b) => [b.textContent, b.getAttribute('aria-pressed'), b.getAttribute('aria-disabled')])"
    r.check('the choices show as buttons', wait(lambda: len(r.s.run(buttons)) == 2 and agent(r)['status'] == 'idle', 15), r.s.run(buttons))
    r.shot('options')
    click(r, 'The last takeClean, 2 s longer') or click(r, 'The last take')
    r.check('a picked choice goes as the next message', wait(lambda: len(runs(r)) == 3 and runs(r)[2]['prompt'].startswith('The last take'), 10)
            and wait(lambda: 'Rozumím: the last take' in panel(r)['text'], 10), runs(r)[-1]['prompt'] if runs(r) else None)
    state = r.s.run(buttons)
    r.check('it is marked chosen and the other is off', state[1][1] == 'true' and state[0][2] == 'true', state)
    idle(r)

    # Stop, in the panel and in the top bar.
    for where in ('Stop', 'Stop and edit'):
        send(r, 'slow edit please')
        r.check(f'a slow edit locks the editor ({where})', wait(lambda: r.state()['aiRun'] and 'Pracuju' in panel(r)['text'], 15))
        pid = runs(r)[-1]['pid']
        if where == 'Stop':
            send(r, 'one more thing')
            r.check('Enter while it works says why nothing was sent', wait(lambda: 'Wait for the answer, or stop it' in panel(r)['text'], 3)
                    and runs(r)[-1]['pid'] == pid and agent(r)['draft'] == 'one more thing', panel(r)['text'])
            r.s.run("window.__nuzky.agent.setState({draft: ''})")
        started = time.time()
        click(r, where, 'aside[aria-label=AI]' if where == 'Stop' else 'header')
        gone = wait(lambda: not alive(pid), 5, 0.05)
        took = round(time.time() - started, 2)
        r.check(f'{where} ends the agent within seconds and unlocks the editor', gone and wait(lambda: not r.state()['aiRun'], 3), {'seconds': took})
        r.check(f'what it did so far stays, and the stopped card says what ({where})',
                wait(lambda: idle(r) and 'Stopped: Slow edit' in panel(r)['text'] and 'Split talk.mp4\n00:02' in panel(r)['text'], 5), panel(r)['text'])
        if where == 'Stop':
            r.shot('stopped')
        r.s.run('window.__nuzky.store.getState().undo()')
        wait(lambda: len(r.track()) == len(before), 5)

    # Problems say what to do next and never lose the message.
    send(r, 'signin please')
    r.check('not signed in says what to do and gives the message back', wait(lambda: "isn't signed in." in panel(r)['text'], 10)
            and agent(r)['draft'].startswith('signin please'), {'draft': agent(r)['draft']})
    r.shot('not-signed-in')
    r.s.run("window.__nuzky.agent.setState({draft: ''})")
    send(r, 'limit please')
    r.check("a used-up plan says so, with the agent's reset time", wait(lambda: 'usage limit reached.' in panel(r)['text'] and 'resets 9pm' in panel(r)['text'], 10), panel(r)['text'])
    r.check('the earlier problem went away with the new message, so its Try again cannot send this one', "isn't signed in." not in panel(r)['text'], panel(r)['text'])
    r.s.run("window.__nuzky.agent.setState({draft: ''})")
    field = "return [window.__nuzky.agent.getState().draft, document.querySelector('aside[aria-label=AI] textarea').value, document.querySelector('aside[aria-label=AI]').innerText.includes('Wait for the answer')]"
    r.check('the field stays as the user left it, with no stale reason under it', wait(lambda: r.s.run(field) == ['', '', False], 3), r.s.run(field))
    r.check('no error toast along the way', not r.errors(), r.errors())

    # Its place: docked left, in the inspector's place, floating; resized; remembered; closed.
    r.s.run(DRAG, 30, 400, False)
    r.check('dragging it to the left edge shows where it would dock', wait(lambda: 'Dock left' in r.s.run('return document.body.innerText'), 3))
    r.shot('dock-preview')
    r.s.run("window.dispatchEvent(new PointerEvent('pointerup', {clientX: 30, clientY: 400, pointerId: 1}))")
    r.check('dropped there, it docks as a column at the left edge', wait(lambda: panel(r)['left'] == 6 and panel(r)['width'] == 360, 3), panel(r))
    r.s.run("document.querySelector('[role=separator][aria-label=\"Resize AI panel\"]').focus()")
    r.key('ArrowRight')
    r.check('the gap beside it resizes it from the keyboard', wait(lambda: panel(r)['width'] == 384, 3), panel(r))
    r.shot('docked-left')
    r.s.run('window.__oldPage = true; location.reload()')
    # Nuzky starts on the home screen; the panel is in the editor behind it.
    wait(lambda: r.s.run("const c = window.__nuzky; if (window.__oldPage || !c?.store.getState().snap) return false; "
                         "c.store.setState({view: 'editor'}); return true", retries=3), 30)
    r.check('after a restart it opens where it was, as wide as it was', wait(lambda: (panel(r) or {}).get('left') == 6 and panel(r)['width'] == 384, 10), panel(r))
    inspector = r.s.run("const b = document.querySelector('aside[aria-label=Inspector]').getBoundingClientRect(); return [b.left + b.width / 2, b.top + b.height / 2]")
    r.s.run(DRAG, inspector[0], inspector[1], True)
    r.check("dropped on the inspector it takes the inspector's place, running down beside the timeline",
            wait(lambda: panel(r)['slot'] == 'inspector' and not panel(r)['inspector'] and panel(r)['width'] == 300 and panel(r)['bottom'] == 6, 3), panel(r))
    r.shot('in-inspector-place')
    r.s.run("document.querySelector('aside[aria-label=AI] button[aria-label=\"Move panel\"]').dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true}))")
    r.check('the grip also opens the places as a menu', wait(lambda: click(r, 'Floating') is not None, 3))
    p = wait(lambda: (lambda p: p if p and p['slot'] is None and p['inspector'] else None)(panel(r)), 3)
    r.check('floating, it sits over the editor and the inspector is back', p, panel(r))
    r.s.run("document.querySelector('aside[aria-label=AI]').parentElement.querySelector('[role=separator]').focus()")
    r.key('ArrowRight')
    r.check('a floating panel resizes from the keyboard too', wait(lambda: panel(r)['width'] == p['width'] + 24, 3), {'before': p['width'], 'after': panel(r)['width']})
    r.shot('floating')
    r.s.run("window.__nuzky.store.setState({view: 'home'})")
    r.check('on the home screen the floating panel does not show over the projects', wait(lambda: (panel(r) or {}).get('width') == 0, 3), panel(r))
    click(r, 'AI', 'div[aria-label=Projects] header')
    r.check("AI on the home screen goes back to the editor with the panel where it was",
            wait(lambda: r.s.run('return window.__nuzky.store.getState().view') == 'editor' and (panel(r) or {}).get('width') == p['width'] + 24, 3), panel(r))
    click(r, 'Close AI (Ctrl+J)')
    r.check('× closes it entirely', wait(lambda: panel(r) is None, 3))
    click(r, 'AI', 'header')
    r.check('AI in the top bar opens it again where it was', wait(lambda: (panel(r) or {}).get('slot') is None and panel(r)['width'] == p['width'] + 24, 3), panel(r))

    # The smallest window has no room for a column beside the editor: it takes the inspector's place.
    r.s.run("document.querySelector('aside[aria-label=AI] button[aria-label=\"Move panel\"]').dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true}))")
    wait(lambda: click(r, 'Dock right') is not None, 3)
    r.check('docked right again from the menu', wait(lambda: panel(r)['right'] == 6 and panel(r)['inspector'], 3), panel(r))
    webdriver('POST', r.s.path + '/window/rect', {'width': 1024, 'height': 640})
    small = wait(lambda: panel(r)['windowWidth'] <= 1030, 5)
    r.check("in a 1024 x 640 window it takes the inspector's place, and goes back once there is room",
            small and wait(lambda: panel(r)['slot'] == 'inspector', 3), panel(r))
    r.shot('small-window')
    webdriver('POST', r.s.path + '/window/rect', {'width': 1440, 'height': 900})
    r.check('back in a wide window it is a column at the right edge again', wait(lambda: panel(r)['right'] == 6 and panel(r)['inspector'], 5), panel(r))
