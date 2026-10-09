"""Update checks. The foot of the home sidebar says when a newer CapOpen was released, from a latest.json served
here in place of GitHub's. The automatic check runs 10 s after start and at most once a day, also across launches,
says nothing when it fails and can be turned off; Check for updates always answers in a toast."""
import gzip, json, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from e2e.harness import flow, wait, webdriver

ROW = """const b = document.querySelector('[data-version]');
return b && {text: b.textContent, focused: document.activeElement === b};"""
MENU = """const m = document.querySelector('[role=menu][aria-label=Updates]');
return m && [...m.querySelectorAll('[role^=menuitem]')].map((i) => ({label: i.textContent, checked: i.getAttribute('aria-checked')}));"""
CHOOSE = """const i = [...document.querySelectorAll('[role=menu][aria-label=Updates] [role^=menuitem]')]
  .find((i) => i.textContent.startsWith(arguments[0]));
if (!i) return false; i.click(); return true;"""
# A click without a pointer, as Enter or Space on the focused row sends it.
OPEN_MENU = "const b = document.querySelector('[data-version]'); b.focus(); b.click();"
IN_MENU = "return !!document.activeElement?.closest('[role=menu][aria-label=Updates]')"
TOASTS = "return window.__capopen.store.getState().toasts.map((t) => ({kind: t.kind, text: t.text}));"
RELOAD = "window.__oldPage = true; location.reload()"
EDITOR_BUTTON = """const b = document.querySelector('[data-update-button]');
return b && {label: b.getAttribute('aria-label'), focused: document.activeElement === b};"""
LOADED = "return !window.__oldPage && !!window.__capopen?.store.getState().snap"


class Server:
    """latest.json as the release would serve it, counting the requests."""
    def __init__(self):
        self.status, self.body, self.requests, self.gzip, self.delay = 200, b'', 0, False, 0
        server = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                server.requests += 1
                time.sleep(server.delay)
                self.send_response(server.status)
                self.send_header('Content-Type', 'application/json')
                if server.gzip:
                    self.send_header('Content-Encoding', 'gzip')
                self.send_header('Content-Length', str(len(server.body)))
                self.end_headers()
                self.wfile.write(server.body)

            def log_message(self, *args):
                pass

        self.http = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=self.http.serve_forever, daemon=True).start()
        self.url = f'http://127.0.0.1:{self.http.server_port}/latest.json'

    def serve(self, version, status=200, pad=0, packed=False):
        """`packed`: gzip, so a large file travels in a few kilobytes."""
        self.status, self.gzip = status, packed
        file = {'version': version, 'pub_date': '2026-10-09T00:00:00Z'}
        if pad:
            file['notes'] = 'x' * pad
        body = json.dumps(file).encode() if status == 200 else b'Not Found'
        self.body = gzip.compress(body) if packed else body


def serve_releases(r):
    r.updates = Server()
    r.updates.serve('99.0.0')
    r.env.pop('CAPOPEN_NO_UPDATE_CHECK', None)
    r.env['CAPOPEN_UPDATE_URL'] = r.updates.url


def row(r):
    return r.s.run(ROW) or {}


def toasts(r):
    return r.s.run(TOASTS)


def manual_check(r):
    """Check for updates from the row's menu; returns the toast it answered with. Earlier toasts go first, as
    an identical toast replaces the one before it."""
    r.s.run('window.__capopen.store.setState({toasts: []})')
    r.s.run(OPEN_MENU)
    if not wait(lambda: r.s.run(MENU), 3) or not r.s.run(CHOOSE, 'Check for updates'):
        return None
    wait(lambda: toasts(r), 15)
    return (toasts(r) or [None])[-1]


@flow('updates', 'the home sidebar says when a newer version is out; checks run daily, quietly, and can be turned off',
      before=serve_releases, home=True)
def updates(r):
    server = r.updates
    try:
        version = r.s.run('return window.__capopen.updates.getState().version')
        r.check('the sidebar foot names this version at start, before anything was asked',
                row(r).get('text') == f'CapOpen {version}' and server.requests == 0, (row(r), server.requests))

        r.check('about 10 s after start it finds the newer version and says so in the sidebar',
                wait(lambda: row(r).get('text') == 'Update available99.0.0', 20) and server.requests == 1, (row(r), server.requests))
        r.check('the automatic check shows no toast', toasts(r) == [], toasts(r))
        r.shot('update-available')

        r.s.run(OPEN_MENU)
        menu = wait(lambda: r.s.run(MENU), 3)
        r.check('its menu offers the download, a check and the automatic check, which is on',
                menu == [{'label': 'Download CapOpen 99.0.0…', 'checked': None}, {'label': 'Check for updates', 'checked': None},
                         {'label': 'Check automatically', 'checked': 'true'}], menu)
        r.check('opened from the keyboard, the menu has the focus', wait(lambda: r.s.run(IN_MENU), 3))
        r.shot('update-menu')
        r.key('Escape')
        r.check('Esc closes the menu and focus returns to the row', wait(lambda: not r.s.run(MENU) and row(r).get('focused'), 3), row(r))

        webdriver('POST', r.s.path + '/window/rect', {'width': 1024, 'height': 640})
        if wait(lambda: r.s.run('return window.innerWidth') == 1024, 5):
            r.check('in a 1024 x 640 window the notice still sits at the foot of the sidebar, in full',
                    r.s.run("""const b = document.querySelector('[data-version]').getBoundingClientRect();
                               const nav = document.querySelector('nav[aria-label=Collections]').getBoundingClientRect();
                               return b.bottom <= window.innerHeight - 6 && b.top >= nav.bottom && b.width > 180;"""))
            r.shot('update-available-small')
        webdriver('POST', r.s.path + '/window/rect', {'width': 1440, 'height': 900})

        r.s.run("window.__capopen.store.setState({view: 'editor'})")
        r.check('in the editor a small button next to Projects says so',
                wait(lambda: r.s.run(EDITOR_BUTTON), 3) == {'label': 'CapOpen 99.0.0 is available', 'focused': False}, r.s.run(EDITOR_BUTTON))
        r.shot('editor-update')
        r.s.run("const b = document.querySelector('[data-update-button]'); b.focus(); b.click();")
        menu = wait(lambda: r.s.run(MENU), 3)
        r.check('it opens the same menu, with the focus in it',
                (menu or [{}])[0].get('label') == 'Download CapOpen 99.0.0…' and wait(lambda: r.s.run(IN_MENU), 3), menu)
        r.shot('editor-update-menu')
        r.key('Escape')
        r.check('Esc closes it and focus returns to the button', wait(lambda: not r.s.run(MENU) and (r.s.run(EDITOR_BUTTON) or {}).get('focused'), 3))
        r.s.run("window.__capopen.store.setState({view: 'home'})")

        server.serve(version)
        answer = manual_check(r)
        r.check('Check for updates answers that this is the latest version, and the notice goes',
                answer == {'kind': 'success', 'text': f'CapOpen {version} is the latest version.'}
                and row(r).get('text') == f'CapOpen {version}' and server.requests == 2, (answer, row(r), server.requests))
        r.s.run("window.__capopen.store.setState({view: 'editor'})")
        r.check('and the editor shows no button', wait(lambda: r.s.run("return !document.querySelector('[data-update-button]') && !!document.querySelector('header')"), 3))
        r.s.run("window.__capopen.store.setState({view: 'home'})")

        server.serve(version, status=404)
        answer = manual_check(r)
        r.check('when the release cannot be read, Check for updates says so and what to try',
                answer == {'kind': 'error', 'text': "Couldn't check for updates. Check your internet connection or try again later."}
                and row(r).get('text') == f'CapOpen {version}', (answer, row(r)))

        server.serve('99.0.0', pad=70_000)
        answer = manual_check(r)
        r.check('a file larger than latest.json can be is not read, even when it names a newer version',
                (answer or {}).get('kind') == 'error' and row(r).get('text') == f'CapOpen {version}', (answer, row(r)))
        server.serve('99.0.0', pad=20_000_000, packed=True)
        answer = manual_check(r)
        r.check('nor is one that arrives small and unpacks large',
                len(server.body) < 65_536 and (answer or {}).get('kind') == 'error' and row(r).get('text') == f'CapOpen {version}',
                (len(server.body), answer, row(r)))
        r.shot('check-failed')

        server.serve('99.0.0')
        asked = server.requests
        r.s.run(RELOAD)
        wait(lambda: r.s.run(LOADED, retries=3), 30)
        time.sleep(13)
        r.check('after a restart within a day it does not ask again', server.requests == asked and row(r).get('text') == f'CapOpen {version}',
                (server.requests, asked, row(r)))

        r.s.run(OPEN_MENU)
        wait(lambda: r.s.run(IN_MENU), 3)
        r.key('End')
        r.key('Enter')
        r.check('chosen from the keyboard, Check automatically turns off and focus returns to the row',
                wait(lambda: not r.s.run(MENU) and row(r).get('focused'), 3)
                and r.s.run('return window.__capopen.updates.getState().auto') is False, row(r))
        r.s.run(OPEN_MENU)
        menu = wait(lambda: r.s.run(MENU), 3)
        r.check('the menu shows it off', (menu or [{}])[-1].get('checked') == 'false', menu)
        r.key('Escape')
        # A day passes.
        r.s.run("const s = JSON.parse(localStorage.getItem('capopen.updates')); s.checkedAt = Date.now() - 25 * 3600e3;"
                "localStorage.setItem('capopen.updates', JSON.stringify(s));" + RELOAD)
        wait(lambda: r.s.run(LOADED, retries=3), 30)
        time.sleep(13)
        r.check('turned off, it does not ask even a day later', server.requests == asked, (server.requests, asked))

        r.s.run(OPEN_MENU)
        wait(lambda: r.s.run(MENU), 3)
        r.s.run(CHOOSE, 'Check automatically')
        r.check('turned on again a day after the last check, it asks at once and finds the newer version',
                wait(lambda: row(r).get('text') == 'Update available99.0.0', 15) and server.requests == asked + 1 and toasts(r) == [],
                (row(r), server.requests, toasts(r)))
        # Another day passes, on a slow connection; Check for updates is chosen while the automatic check runs.
        asked, server.delay = server.requests, 3
        r.s.run("const s = JSON.parse(localStorage.getItem('capopen.updates')); s.checkedAt = Date.now() - 25 * 3600e3;"
                "localStorage.setItem('capopen.updates', JSON.stringify(s));" + RELOAD)
        wait(lambda: r.s.run(LOADED, retries=3), 30)
        wait(lambda: server.requests == asked + 1, 15)
        r.s.call('window.__capopen.checkForUpdates(true)')
        r.check('Check for updates during the automatic check answers once it is done, without asking twice',
                wait(lambda: toasts(r), 10) and toasts(r)[-1] == {'kind': 'info', 'text': 'CapOpen 99.0.0 is available.'}
                and server.requests == asked + 1, (toasts(r), server.requests, asked))
        server.delay = 0
        r.check('nothing went wrong on the way', [t for t in toasts(r) if t['kind'] == 'error'] == [], toasts(r))
    finally:
        server.http.shutdown()
