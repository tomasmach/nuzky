"""Shared parts of the end-to-end flows: the app under WebDriver, isolated directories, checks and proof.

A flow is a function decorated with @flow in its own file under tests/e2e/. It gets a Run, drives the real
app through the dev-only `window.__capopen` hook and the keyboard, and records what the user would see
with r.check(...). Run flows with scripts/repro.py.
"""
import base64, datetime, json, os, queue, shutil, signal, socket, subprocess, tempfile, threading, time
import traceback, urllib.error, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / 'tmp-test'
MODELS = FIXTURES / 'xdg/data/capopen/models'
OUT = ROOT / 'tmp-test/repro'
TARGET = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
APP, CLI = TARGET / 'debug/capopen-app', TARGET / 'debug/capopen'
WEBKIT_DRIVER = shutil.which('WebKitWebDriver') or '/usr/bin/WebKitWebDriver'
AI_EDITING = 'AI is editing. Stop it to edit yourself.'
FLOWS = {}


def flow(name, description, before=None):
    def register(fn):
        FLOWS[name] = (description, before, fn)
        return fn
    return register


# --- WebDriver client for the app's webview, straight to WebKitWebDriver --------------------------------

def webdriver(method, path, body=None, retries=0):
    """Retries only queries, which are safe to send twice when a connection drops."""
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request('http://127.0.0.1:4444' + path, data=data, method=method,
                                     headers={'Content-Type': 'application/json'})
    for attempt in range(retries + 1):
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return json.loads(response.read() or b'{}').get('value')
        except urllib.error.HTTPError as e:
            raise RuntimeError(f'WebDriver {e.code}: {e.read()[:500]!r}') from None
        except (ConnectionResetError, ConnectionRefusedError):
            if attempt == retries:
                raise
            time.sleep(0.3)


class Session:
    def __init__(self):
        # What tauri-driver sends on Linux. Talking to WebKitWebDriver directly avoids tauri-driver's connection
        # pool, whose stale connections now and then dropped a request mid-flow.
        caps = {'capabilities': {'alwaysMatch': {'browserName': 'wry',
                                                 'webkitgtk:browserOptions': {'binary': str(APP), 'args': []}}}}
        self.path = '/session/' + webdriver('POST', '/session', caps)['sessionId']

    def run(self, script, *args, retries=0):
        return webdriver('POST', self.path + '/execute/sync', {'script': script, 'args': list(args)}, retries)

    def call(self, expression, *args):
        """Awaits a promise in the page; returns {'ok': True, 'value': …} or {'ok': False, 'error': …}."""
        script = ('const done = arguments[arguments.length - 1];'
                  f'Promise.resolve().then(() => {expression})'
                  '.then((v) => done({ok: true, value: typeof v === "string" || typeof v === "number" ? v : null}))'
                  '.catch((e) => done({ok: false, error: String(e)}))')
        return webdriver('POST', self.path + '/execute/async', {'script': script, 'args': list(args)})

    def screenshot(self, path):
        path.write_bytes(base64.b64decode(webdriver('GET', self.path + '/screenshot', retries=3)))

    def close(self):
        try:
            webdriver('DELETE', self.path)
        except Exception:
            pass


def wait(fn, timeout=30, step=0.2):
    end = time.time() + timeout
    while True:
        value = fn()
        if value or time.time() > end:
            return value
        time.sleep(step)


# --- One flow: its directories, processes, checks and proof -------------------------------------------

STATE = """const st = window.__capopen.store.getState(); const p = st.snap.project;
return {
  assets: p.assets.map((a) => ({id: a.id, name: a.name})),
  tracks: p.tracks.map((t) => ({id: t.id, kind: t.kind, name: t.name, clips: [...t.clips]
    .sort((a, b) => a.startUs - b.startUs)
    .map((c) => ({id: c.id, startUs: c.startUs, durationUs: c.durationUs, assetId: c.content.assetId ?? null,
                  text: c.content.text ?? null}))})),
  thumbs: Object.keys(st.thumbs), aiRun: st.aiRun, epoch: st.snap.sessionEpoch,
  toasts: st.toasts.map((t) => ({kind: t.kind, text: t.text})),
  jobs: Object.values(st.jobs).map((j) => ({kind: j.kind, status: j.status, message: j.message})),
};"""
KEY = """const target = document.activeElement || document.body;
target.dispatchEvent(new KeyboardEvent('keydown', Object.assign({key: arguments[0], bubbles: true, cancelable: true}, arguments[1])));"""


class Run:
    def __init__(self, name, runtime):
        self.name, self.work = name, OUT / name
        shutil.rmtree(self.work, ignore_errors=True)
        (self.work / 'data/capopen/projects').mkdir(parents=True)
        self.env = dict(os.environ, XDG_DATA_HOME=str(self.work / 'data'), XDG_CACHE_HOME=str(self.work / 'cache'),
                        XDG_RUNTIME_DIR=runtime, DBUS_SESSION_BUS_ADDRESS='disabled:', GDK_BACKEND='x11',
                        PYTHONDONTWRITEBYTECODE='1')
        self.checks, self.shots, self.s = [], [], None

    def check(self, name, ok, detail=None):
        self.checks.append({'check': name, 'ok': bool(ok), 'detail': detail})
        print(f"  {'ok  ' if ok else 'FAIL'} {name}" + ('' if ok or detail is None else f': {detail}'), flush=True)
        return bool(ok)

    def shot(self, name):
        self.s.screenshot(self.work / f'{name}.png')
        if f'{name}.png' not in self.shots:
            self.shots.append(f'{name}.png')

    def state(self):
        return self.s.run(STATE, retries=3)

    def track(self, track_id='main'):
        return next(t['clips'] for t in self.state()['tracks'] if t['id'] == track_id)

    def key(self, key, **modifiers):
        self.s.run(KEY, key, modifiers)

    def focus_clip(self, clip_id):
        self.s.run("document.querySelector(`[data-clip-id='${arguments[0]}']`).focus();"
                   "window.__capopen.store.getState().select([arguments[0]]);", clip_id)

    def seek(self, us):
        self.s.run('window.__capopen.store.getState().seek(arguments[0])', us)

    def import_media(self, *paths):
        count = len(self.state()['assets']) + len(paths)
        self.s.call('window.__capopen.importPaths(arguments[0])', [str(p) for p in paths])
        if not wait(lambda: len(self.state()['assets']) == count):
            raise RuntimeError(f'import did not finish: {self.state()["assets"]}')

    def add_clip(self, name):
        count = len(self.track())
        added = self.s.call("""(() => { const st = window.__capopen.store.getState();
            const asset = st.snap.project.assets.find((a) => a.name === arguments[0]);
            return st.edit({type: 'addClip', trackId: 'main', assetId: asset.id, startUs: null}); })()""", name)
        if not added['ok'] or not wait(lambda: len(self.track()) == count + 1):
            raise RuntimeError(f'{name} was not placed: {added}')

    def errors(self):
        return [t['text'] for t in self.state()['toasts'] if t['kind'] == 'error']

    def saved_project(self):
        return next((self.work / 'data/capopen/projects').glob('*.capopen'))


def start(command, log, env=None):
    return subprocess.Popen(command, cwd=ROOT, env=env, stdout=open(log, 'w'), stderr=subprocess.STDOUT,
                            start_new_session=True)


def stop(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(5)
    except ProcessLookupError:
        pass
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)


def port_busy(port):
    """Vite may listen on ::1 only, so every address of localhost counts."""
    try:
        socket.create_connection(('localhost', port), timeout=1).close()
        return True
    except OSError:
        return False


def run_flow(name):
    description, before, body = FLOWS[name]
    print(f'\n==> {name}: {description}', flush=True)
    runtime = tempfile.mkdtemp(prefix='capopen-repro-')  # Unix socket paths must stay short.
    r = Run(name, runtime)
    error = driver = None
    try:
        if before:
            before(r)
        # A WebDriver that is not ours would launch the app outside this run's directories.
        if not wait(lambda: not port_busy(4444), 10):
            raise RuntimeError('port 4444 is in use by another session')
        # Tauri 2 lets WebDriver drive its webview only with this set, as tauri-driver does.
        driver = start([WEBKIT_DRIVER, '--port=4444', '--host=127.0.0.1'], r.work / 'driver.log',
                       dict(r.env, TAURI_WEBVIEW_AUTOMATION='true'))
        if not wait(lambda: port_busy(4444), 20):
            raise RuntimeError('WebKitWebDriver did not start, see driver.log')
        r.s = Session()
        if not wait(lambda: r.s.run('return !!window.__capopen?.store.getState().snap', retries=3), 60):
            raise RuntimeError('the app did not load a project')
        time.sleep(1)
        body(r)
    except Exception:
        error = traceback.format_exc()
        print(error, flush=True)
    finally:
        if r.s:
            r.s.close()
        if driver:
            stop(driver)
        shutil.rmtree(runtime, ignore_errors=True)
    revision = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    dirty = bool(subprocess.run(['git', 'status', '--porcelain'], cwd=ROOT, capture_output=True, text=True).stdout)
    result = {'flow': name, 'description': description, 'passed': not error and bool(r.checks) and all(c['ok'] for c in r.checks),
              'checks': r.checks, 'error': error, 'screenshots': r.shots, 'command': f'python3 scripts/repro.py {name}',
              'revision': revision, 'uncommitted_changes': dirty, 'finished': datetime.datetime.now().isoformat(timespec='seconds')}
    (r.work / 'result.json').write_text(json.dumps(result, indent=2, ensure_ascii=False, default=lambda o: sorted(o) if isinstance(o, set) else str(o)) + '\n')


# --- Helpers shared by flows ----------------------------------------------------------------------------

def preview_rect(r):
    return r.s.run("""const b = document.querySelector('section[aria-label=Preview] canvas').getBoundingClientRect();
        return {left: b.left, top: b.top, width: b.width, height: b.height, viewport: window.innerWidth};""")


def preview_crop(screenshot, rect):
    from PIL import Image
    image = Image.open(screenshot).convert('RGB')
    scale = image.width / rect['viewport']
    left, top = int(rect['left'] * scale), int(rect['top'] * scale)
    return image.crop((left, top, left + int(rect['width'] * scale), top + int(rect['height'] * scale)))


def preview_redraw(r, name, before, timeout=6):
    """Screenshots the preview until it differs from `before` (a crop), so the picture follows the last edit.
    Returns the crop and whether the preview changed in time."""
    rect = preview_rect(r)
    end = time.time() + timeout
    while True:
        r.shot(name)
        crop = preview_crop(r.work / f'{name}.png', rect)
        if changed_share(crop, before) > 0:
            return crop, True
        if time.time() > end:
            return crop, False
        time.sleep(0.5)


def preview_brightness(r, screenshot, fraction):
    """Mean brightness, 0 to 255, of the top `fraction` of the preview."""
    from PIL import ImageStat
    crop = preview_crop(screenshot, preview_rect(r))
    band = crop.crop((0, 0, crop.width, max(1, int(crop.height * fraction)))).convert('L')
    return ImageStat.Stat(band).mean[0]


def ffprobe(path):
    out = subprocess.run(['ffprobe', '-v', 'error', '-show_entries', 'format=duration:stream=codec_name', '-of', 'json', path],
                         capture_output=True, text=True, check=True).stdout
    data = json.loads(out)
    return float(data['format']['duration']), {s['codec_name'] for s in data['streams']}


def changed_share(a, b):
    """Share of pixels whose brightness clearly differs, for two image paths or Pillow images. Encoding noise
    stays below the threshold; the neighbouring frame of the test pattern changes about 1 % of the picture."""
    from PIL import Image, ImageChops, ImageStat
    first, second = (Image.open(x) if isinstance(x, (str, Path)) else x for x in (a, b))
    first, second = first.convert('L'), second.convert('L')
    if first.size != second.size:
        return 1.0
    return ImageStat.Stat(ImageChops.difference(first, second).point(lambda v: 255 if v > 48 else 0)).mean[0] / 255


def close_window_and_confirm(r):
    """Closes the app window like the title-bar button and confirms the native quit question with Enter."""
    from Xlib import X, XK, display
    from Xlib.protocol import event
    d = display.Display()

    def windows():
        found, stack = [], [d.screen().root]
        while stack:
            w = stack.pop()
            try:
                name = w.get_wm_name()
                stack.extend(w.query_tree().children)
            except Exception:
                continue
            if name:
                found.append((w, name.decode() if isinstance(name, bytes) else name))
        return found

    protocols, delete = d.intern_atom('WM_PROTOCOLS'), d.intern_atom('WM_DELETE_WINDOW')
    for w, name in windows():
        if name == 'CapOpen':
            w.send_event(event.ClientMessage(window=w, client_type=protocols, data=(32, [delete, X.CurrentTime, 0, 0, 0])))
    d.sync()
    dialog = wait(lambda: next((w for w, name in windows() if 'Quit CapOpen' in name), None), 5, 0.1)
    if not dialog:
        return False
    time.sleep(0.8)
    geometry = dialog.get_geometry()
    raw = dialog.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xffffffff)
    from PIL import Image
    Image.frombytes('RGB', (geometry.width, geometry.height), raw.data, 'raw', 'BGRX').save(r.work / 'quit-question.png')
    r.shots.append('quit-question.png')
    code = d.keysym_to_keycode(XK.string_to_keysym('Return'))
    for kind in (event.KeyPress, event.KeyRelease):
        dialog.send_event(kind(time=X.CurrentTime, root=d.screen().root, window=dialog, same_screen=1, child=X.NONE,
                               root_x=0, root_y=0, event_x=0, event_y=0, state=0, detail=code), propagate=True)
    d.sync()
    return True


class Bridge:
    """The agent's side: `capopen mcp` attached to the open app over its local socket."""

    def __init__(self, r, project):
        self.process = subprocess.Popen([str(CLI), 'mcp', '--project', str(project), '--allow-write'], env=r.env,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(r.work / 'bridge.log', 'w'),
                                        text=True, start_new_session=True)
        self.lines, self.id = queue.Queue(), 0
        threading.Thread(target=lambda: [self.lines.put(line) for line in self.process.stdout], daemon=True).start()
        self.rpc('initialize', {'protocolVersion': '2025-03-26', 'capabilities': {}, 'clientInfo': {'name': 'repro', 'version': '1'}})
        self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + '\n')
        self.process.stdin.flush()

    def rpc(self, method, params):
        self.id += 1
        self.send({'jsonrpc': '2.0', 'id': self.id, 'method': method, 'params': params})
        while True:
            message = json.loads(self.lines.get(timeout=30))
            if message.get('id') == self.id:
                return message

    def call(self, tool, arguments):
        response = self.rpc('tools/call', {'name': tool, 'arguments': arguments})
        if 'error' in response or response['result'].get('isError'):
            raise RuntimeError(f'{tool} failed: {response}')
        return response['result']['structuredContent']

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(10)
        except subprocess.TimeoutExpired:
            stop(self.process)


def link_models(r):
    models = r.work / 'data/capopen/models'
    models.mkdir(parents=True)
    for name in ('ggml-small.bin', 'ggml-silero-v5.1.2.bin'):
        try:
            os.link(MODELS / name, models / name)
        except OSError:
            shutil.copy(MODELS / name, models / name)
