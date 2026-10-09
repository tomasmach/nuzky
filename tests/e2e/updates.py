"""Update checks. The foot of the home sidebar says when a newer CapOpen was released, from a latest.json served
here in place of GitHub's. The automatic check runs 10 s after start and at most once a day, also across launches,
says nothing when it fails and can be turned off; Check for updates always answers in a toast."""
import json, threading, time
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
TOASTS = "return window.__capopen.store.getState().toasts.map((t) => ({kind: t.kind, text: t.text}));"
RELOAD = "window.__oldPage = true; location.reload()"
LOADED = "return !window.__oldPage && !!window.__capopen?.store.getState().snap"


class Server:
    """latest.json as the release would serve it, counting the requests."""
    def __init__(self):
        self.status, self.body, self.requests = 200, b'', 0
        server = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                server.requests += 1
                self.send_response(server.status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(server.body)))
                self.end_headers()
                self.wfile.write(server.body)

            def log_message(self, *args):
                pass

        self.http = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=self.http.serve_forever, daemon=True).start()
        self.url = f'http://127.0.0.1:{self.http.server_port}/latest.json'

    def serve(self, version, status=200, pad=0):
        self.status = status
        file = {'version': version, 'pub_date': '2026-10-09T00:00:00Z'}
        if pad:
            file['notes'] = 'x' * pad
        self.body = json.dumps(file).encode() if status == 200 else b'Not Found'


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
        r.shot('update-menu')
        r.s.run("document.querySelector('[role=menu][aria-label=Updates]').dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}))")
        r.check('Esc closes the menu and focus returns to the row', wait(lambda: not r.s.run(MENU) and row(r).get('focused'), 3), row(r))

        webdriver('POST', r.s.path + '/window/rect', {'width': 1024, 'height': 640})
        if wait(lambda: r.s.run('return window.innerWidth') == 1024, 5):
            r.check('in a 1024 x 640 window the notice still sits at the foot of the sidebar, in full',
                    r.s.run("""const b = document.querySelector('[data-version]').getBoundingClientRect();
                               const nav = document.querySelector('nav[aria-label=Collections]').getBoundingClientRect();
                               return b.bottom <= window.innerHeight - 6 && b.top >= nav.bottom && b.width > 180;"""))
            r.shot('update-available-small')
        webdriver('POST', r.s.path + '/window/rect', {'width': 1440, 'height': 900})

        server.serve(version)
        answer = manual_check(r)
        r.check('Check for updates answers that this is the latest version, and the notice goes',
                answer == {'kind': 'success', 'text': f'CapOpen {version} is the latest version.'}
                and row(r).get('text') == f'CapOpen {version}' and server.requests == 2, (answer, row(r), server.requests))

        server.serve(version, status=404)
        answer = manual_check(r)
        r.check('when the release cannot be read, Check for updates says so and what to try',
                answer == {'kind': 'error', 'text': "Couldn't check for updates. Check your internet connection and try again."}
                and row(r).get('text') == f'CapOpen {version}', (answer, row(r)))

        server.serve('99.0.0', pad=70_000)
        answer = manual_check(r)
        r.check('a file larger than latest.json can be is not read, even when it names a newer version',
                (answer or {}).get('kind') == 'error' and row(r).get('text') == f'CapOpen {version}', (answer, row(r)))
        r.shot('check-failed')

        server.serve('99.0.0')
        asked = server.requests
        r.s.run(RELOAD)
        wait(lambda: r.s.run(LOADED, retries=3), 30)
        time.sleep(13)
        r.check('after a restart within a day it does not ask again', server.requests == asked and row(r).get('text') == f'CapOpen {version}',
                (server.requests, asked, row(r)))

        r.s.run(OPEN_MENU)
        wait(lambda: r.s.run(MENU), 3)
        r.s.run(CHOOSE, 'Check automatically')
        r.s.run(OPEN_MENU)
        menu = wait(lambda: r.s.run(MENU), 3)
        r.check('Check automatically turns off', (menu or [{}])[-1].get('checked') == 'false', menu)
        r.s.run("document.querySelector('[role=menu][aria-label=Updates]').dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}))")
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
        r.check('nothing went wrong on the way', [t for t in toasts(r) if t['kind'] == 'error'] == [], toasts(r))
    finally:
        server.http.shutdown()
