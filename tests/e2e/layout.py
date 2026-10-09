import subprocess

from e2e.harness import CLI, FIXTURES, flow, wait, webdriver

SIZES = """const width = (e) => e ? Math.round(e.getBoundingClientRect().width) : null;
return {library: width(document.querySelector('[role=tablist][aria-label=Library]').closest('aside')),
        inspector: width(document.querySelector('aside[aria-label=Inspector]')),
        preview: width(document.querySelector('section[aria-label=Preview]')),
        timeline: Math.round(document.querySelector('section[aria-label=Timeline]').getBoundingClientRect().height),
        window: [window.innerWidth, window.innerHeight]};"""
GAP = "document.querySelector(`[role=separator][aria-label='Resize ${arguments[0]}']`)"
DRAG = GAP + """; const b = s.getBoundingClientRect();
const at = (type, dx, dy) => new PointerEvent(type, {bubbles: true, button: 0, pointerId: 1,
  clientX: b.left + b.width / 2 + dx, clientY: b.top + b.height / 2 + dy});
s.dispatchEvent(at('pointerdown', 0, 0));
window.dispatchEvent(at('pointermove', arguments[1], arguments[2]));
window.dispatchEvent(at('pointerup', arguments[1], arguments[2]));"""
COLUMNS = """const tile = [...document.querySelectorAll('[role=tablist][aria-label=Library] ~ [role=tabpanel] *')].find((e) => e.textContent === 'talk.mp4');
return getComputedStyle(tile.closest('.grid')).gridTemplateColumns.split(' ').length;"""


def setup(r):
    project = r.work / 'data/nuzky/projects/layout.nuzky'
    subprocess.run([str(CLI), 'new', str(project), str(FIXTURES / 'talk.mp4')], env=r.env, check=True, capture_output=True)


def sizes(r):
    return r.s.run(SIZES, retries=3)


def drag(r, what, dx, dy=0):
    r.s.run('const s = ' + DRAG, what, dx, dy)


def press(r, what, key):
    r.s.run(GAP + '.focus()', what)
    r.key(key)


def resize(r, width, height):
    webdriver('POST', r.s.path + '/window/rect', {'width': width, 'height': height})
    return wait(lambda: sizes(r)['window'] == [width, height], 5)


@flow('layout', "The library, inspector and timeline start in CapCut's proportions, resize from the gaps between them by "
      'pointer and keyboard, never squeeze the preview under 340 px, and are remembered',
      before=setup)
def layout(r):
    first = sizes(r)
    columns = r.s.run(COLUMNS)
    w, h = first['window']
    r.check("by default the library and the inspector take 28 % of the window's width and the timeline 35 % of its height",
            first['library'] == round(w * 0.28) and first['inspector'] == round(w * 0.28) and first['timeline'] == round(h * 0.35), first)
    r.shot('default')

    drag(r, 'library', 100)
    s = sizes(r)
    r.check('dragging the gap right of the library widens it, and only the preview gives way',
            s['library'] == first['library'] + 100 and s['inspector'] == first['inspector'] and s['preview'] == first['preview'] - 100, s)
    r.seek(1_000_000)
    wait(lambda: r.s.run('return window.__nuzky.store.getState().timeUs') == 1_000_000, 5)
    press(r, 'inspector', 'ArrowLeft')
    s = sizes(r)
    r.check('←, with the gap left of the inspector focused, widens the inspector by 24 px', s['inspector'] == first['inspector'] + 24, s)
    r.check('the arrow did not move the playhead', r.s.run('return window.__nuzky.store.getState().timeUs') == 1_000_000)
    drag(r, 'timeline', 0, -60)
    press(r, 'timeline', 'ArrowUp')
    s = sizes(r)
    r.check('dragging the gap above the timeline up and ↑ make the timeline taller', s['timeline'] == first['timeline'] + 84, s)
    drag(r, 'library', 2000)
    wide = sizes(r)
    r.check('the library stops where the preview has 340 px left, and the inspector keeps its width',
            wide['preview'] == 340 and wide['inspector'] == first['inspector'] + 24, wide)
    tabs = r.s.run("return [...document.querySelectorAll('[role=tablist][aria-label=Library] [role=tab]')].map((t) => t.getBoundingClientRect().width)")
    r.check('the library tabs share the extra width', min(tabs) > 50, tabs)
    r.check('the wider library shows more media in a row instead of larger tiles', columns == 3 and r.s.run(COLUMNS) >= 5, [columns, r.s.run(COLUMNS)])
    r.shot('resized')

    r.s.run('window.__oldPage = true; location.reload()')
    wait(lambda: r.s.run("const c = window.__nuzky; if (window.__oldPage || !c?.store.getState().snap) return false; "
                         "c.store.setState({view: 'editor'}); return true", retries=3), 30)
    r.check('after a restart every pane is as the user left it', wait(lambda: sizes(r) == wide, 10), sizes(r))
    r.s.run(GAP + ".dispatchEvent(new MouseEvent('dblclick', {bubbles: true}))", 'library')
    s = sizes(r)
    r.check('double-clicking the gap puts the library back to its share of the window', s['library'] == first['library'], s)

    if r.check('the window gets to 1024 x 640', resize(r, 1024, 640)):
        # The panes follow on the render after the resize event; the library, back on its share, shows it.
        wait(lambda: sizes(r)['library'] == 360, 5)
        small = sizes(r)
        r.check('in the smallest window the inspector gives way first and the preview keeps 340 px',
                small['preview'] == 340 and small['library'] == 360 and small['inspector'] == 300, small)
        r.check('the timeline leaves 300 px above it for the preview', small['timeline'] == 640 - 48 - 300 - 12, small)
        r.shot('small-window')
        resize(r, w, h)
        r.check('back in the large window the inspector and the timeline are as the user left them',
                wait(lambda: (lambda s: s['inspector'] == wide['inspector'] and s['timeline'] == wide['timeline'])(sizes(r)), 5), sizes(r))

    r.check('no error toast', not r.errors(), r.errors())
