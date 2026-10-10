"""Shared parts of the end-to-end flows: the app under WebDriver, isolated directories, checks and proof.

A flow is a function decorated with @flow in its own file under tests/e2e/. It gets a Run, drives the real
app through the dev-only `window.__nuzky` hook and the keyboard, and records what the user would see
with r.check(...). Run flows with scripts/repro.py.

On Linux WebKitWebDriver drives the app. macOS has no WebDriver for an app's webview, so there the debug app
answers the same requests itself, over a private socket, when the harness starts it with a secret
(src-tauri/src/test_bridge.rs).
"""
import base64, datetime, json, os, queue, secrets, shutil, signal, socket, subprocess, sys, tempfile, threading, time
import traceback, urllib.error, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / 'tmp-test'
MODELS = FIXTURES / 'xdg/data/nuzky/models'
OUT = ROOT / 'tmp-test/repro'
TARGET = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
APP, CLI, ANALYZE = TARGET / 'debug/nuzky-app', TARGET / 'debug/nuzky', TARGET / 'debug/nuzky-analyze'
WEBKIT_DRIVER = shutil.which('WebKitWebDriver') or '/usr/bin/WebKitWebDriver'
AI_EDITING = 'AI is editing. Stop it to edit yourself.'
FLOWS = {}
MACOS = sys.platform == 'darwin'
RUN = None  # The flow in progress; on macOS its app, socket and secret.


def flow(name, description, before=None, home=False):
    """`home`: the flow starts on the home screen; others start in the editor of the restored project."""
    def register(fn):
        fn.home = home
        FLOWS[name] = (description, before, fn)
        return fn
    return register


# --- WebDriver client for the app's webview, straight to WebKitWebDriver --------------------------------

def webdriver(method, path, body=None, retries=0):
    """Retries only queries, which are safe to send twice when a connection drops."""
    if MACOS:
        return _bridge(method, path, body, retries)
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


def _bridge(method, path, body, retries):
    """The same requests to the app's own socket on macOS, one JSON line each way."""
    request = (json.dumps({'token': RUN.token, 'method': method, 'path': path, 'body': body}) + '\n').encode()
    for attempt in range(retries + 1):
        try:
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(130)
                connection.connect(str(RUN.socket))
                connection.sendall(request)
                reply = json.loads(connection.makefile().readline() or '{"error": "the app closed the connection"}')
            break
        except (ConnectionResetError, ConnectionRefusedError, FileNotFoundError):
            if attempt == retries:
                raise
            time.sleep(0.3)
    if 'error' in reply:
        raise RuntimeError(f'WebDriver: {reply["error"][:500]}')
    return reply['value']


class Session:
    def __init__(self):
        if MACOS:
            # The app itself, started for each session as WebKitWebDriver starts it on Linux.
            RUN.socket.unlink(missing_ok=True)
            RUN.app = start([str(APP)], RUN.work / 'app.log', dict(RUN.env, NUZKY_TEST_BRIDGE=RUN.token))
            if not wait(lambda: RUN.app.poll() is not None or RUN.socket.exists(), 60) or RUN.app.poll() is not None:
                raise RuntimeError('the app did not start, see app.log')
            caps = {}
        else:
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
        if MACOS:
            stop(RUN.app)


def wait(fn, timeout=30, step=0.2):
    end = time.time() + timeout
    while True:
        value = fn()
        if value or time.time() > end:
            return value
        time.sleep(step)


# --- One flow: its directories, processes, checks and proof -------------------------------------------

STATE = """const st = window.__nuzky.store.getState(); const p = st.snap.project;
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
        (self.work / 'data/nuzky/projects').mkdir(parents=True)
        self.env = dict(os.environ, XDG_DATA_HOME=str(self.work / 'data'), XDG_CACHE_HOME=str(self.work / 'cache'),
                        XDG_RUNTIME_DIR=runtime, DBUS_SESSION_BUS_ADDRESS='disabled:', GDK_BACKEND='x11',
                        # Flows never ask GitHub for a new version; tests/e2e/updates.py serves its own.
                        NUZKY_NO_UPDATE_CHECK='1',
                        PYTHONDONTWRITEBYTECODE='1')
        self.checks, self.shots, self.s, self.app = [], [], None, None
        if MACOS:
            # macOS keeps Nuzky's data and cache under ~/Library and WebKit's storage under the home folder, whatever
            # XDG_* say, so the app gets a home of its own whose Library folders are this run's.
            home = self.work / 'home'
            for folder in (home / 'Library/Application Support', home / 'Library/Caches', self.work / 'cache/nuzky'):
                folder.mkdir(parents=True)
            os.symlink(self.work / 'data/nuzky', home / 'Library/Application Support/nuzky')
            os.symlink(self.work / 'cache/nuzky', home / 'Library/Caches/nuzky')
            self.env.update(HOME=str(home), CFFIXED_USER_HOME=str(home))
            self.socket, self.token = Path(runtime) / 'nuzky/webdriver.sock', secrets.token_hex(32)

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
                   "window.__nuzky.store.getState().select([arguments[0]]);", clip_id)

    def seek(self, us):
        self.s.run('window.__nuzky.store.getState().seek(arguments[0])', us)

    def import_media(self, *paths):
        count = len(self.state()['assets']) + len(paths)
        self.s.call('window.__nuzky.importPaths(arguments[0])', [str(p) for p in paths])
        if not wait(lambda: len(self.state()['assets']) == count):
            raise RuntimeError(f'import did not finish: {self.state()["assets"]}')

    def add_clip(self, name):
        count = len(self.track())
        added = self.s.call("""(() => { const st = window.__nuzky.store.getState();
            const asset = st.snap.project.assets.find((a) => a.name === arguments[0]);
            return st.edit({type: 'addClip', trackId: 'main', assetId: asset.id, startUs: null}); })()""", name)
        if not added['ok'] or not wait(lambda: len(self.track()) == count + 1):
            raise RuntimeError(f'{name} was not placed: {added}')

    def errors(self):
        return [t['text'] for t in self.state()['toasts'] if t['kind'] == 'error']

    def saved_project(self):
        return next((self.work / 'data/nuzky/projects').glob('*.nuzky'))


def start(command, log, env=None):
    return subprocess.Popen(command, cwd=ROOT, env=env, stdout=open(log, 'w'), stderr=subprocess.STDOUT,
                            start_new_session=True)


def stop(process, timeout=5):
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout)
    except ProcessLookupError:
        pass
    except PermissionError:
        # macOS refuses to signal a group whose leader has ended unwaited, such as the app after its window closed.
        process.wait(timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)


def started(process, port, timeout):
    """True once `port` answers while `process` still runs; a server that exited left the port to someone else."""
    wait(lambda: process.poll() is not None or port_busy(port), timeout)
    return process.poll() is None and port_busy(port)


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
    # Unix socket paths must stay short, and macOS's TMPDIR is long.
    runtime = tempfile.mkdtemp(prefix='nuzky-repro-', dir='/tmp' if MACOS else None)
    global RUN
    r = RUN = Run(name, runtime)
    error = driver = None
    try:
        if before:
            before(r)
        if not MACOS:
            # A WebDriver that is not ours would launch the app outside this run's directories.
            if not wait(lambda: not port_busy(4444), 10):
                raise RuntimeError('port 4444 is in use by another session')
            # Tauri 2 lets WebDriver drive its webview only with this set, as tauri-driver does.
            driver = start([WEBKIT_DRIVER, '--port=4444', '--host=127.0.0.1'], r.work / 'driver.log',
                           dict(r.env, TAURI_WEBVIEW_AUTOMATION='true'))
            if not started(driver, 4444, 20):
                raise RuntimeError('WebKitWebDriver did not start, see driver.log')
        r.s = Session()
        if not wait(lambda: r.s.run('return !!window.__nuzky?.store.getState().snap', retries=3), 60):
            raise RuntimeError('the app did not load a project')
        if not body.home:
            r.s.run("window.__nuzky.store.setState({view: 'editor'})")
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
        if r.app:
            stop(r.app)
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


def export(r, name):
    """Exports the timeline at 720p to `name` in the flow's folder and checks that the job finishes."""
    target = r.work / name
    started = r.s.call('window.__nuzky.api.startExport(arguments[0], arguments[1], arguments[2], arguments[3])',
                       str(target), {'resolution': 720, 'fps': 30, 'quality': 'small'}, r.state()['epoch'], True)
    if not started['ok']:
        raise RuntimeError(f'export {name} did not start: {started}')
    job = wait(lambda: (j := r.s.run("return window.__nuzky.store.getState().jobs[arguments[0]] ?? null", started['value']))
               and j['status'] != 'running' and j, 180)
    r.check(f'the export {name} finishes', job and job['status'] == 'done', job)
    return target


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


def _windows(d):
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


def press(key):
    """A real key press into the app window, an X11 key name or a character: a focused button acts on Enter and
    focus moves on Tab as they do for a person, which key events made by the page do not do. Through XTest under
    gamescope, through the window itself on macOS; WebKitWebDriver has no key input."""
    if MACOS:
        webdriver('POST', '/session/nuzky/nuzky/keys', {'keys': [key]})
        return
    from Xlib import X, XK, display
    from Xlib.ext import xtest
    d = display.Display()
    for window, name in _windows(d):
        if name == 'Nuzky':
            window.set_input_focus(X.RevertToParent, X.CurrentTime)
    code = d.keysym_to_keycode(XK.string_to_keysym(key))
    for kind in (X.KeyPress, X.KeyRelease):
        xtest.fake_input(d, kind, code)
    d.sync()
    d.close()


def pointer(kind, x, y, clicks=1):
    """macOS: the left mouse button `down` or `up` at a point of the page."""
    webdriver('POST', '/session/nuzky/nuzky/pointer', {'kind': kind, 'x': x, 'y': y, 'clicks': clicks})


def close_window(d=None):
    """Closes the app window like the title-bar button."""
    if MACOS:
        webdriver('DELETE', '/session/nuzky/window')
        return None
    from Xlib import X, display
    from Xlib.protocol import event
    d = d or display.Display()
    protocols, delete = d.intern_atom('WM_PROTOCOLS'), d.intern_atom('WM_DELETE_WINDOW')
    for w, name in _windows(d):
        if name == 'Nuzky':
            w.send_event(event.ClientMessage(window=w, client_type=protocols, data=(32, [delete, X.CurrentTime, 0, 0, 0])))
    d.sync()
    return d


def close_window_and_confirm(r):
    """Closes the app window like the title-bar button and confirms the native quit question with Enter."""
    if MACOS:
        close_window()

        def question():
            try:
                return any('Quit Nuzky' in text for text in webdriver('GET', r.s.path + '/alert/text'))
            except RuntimeError:
                return False
        # AppKit draws the dialog's glass only on screen, so the proof here is its text, not a picture.
        if not wait(question, 5, 0.1):
            return False
        (r.work / 'quit-question.txt').write_text('\n'.join(webdriver('GET', r.s.path + '/alert/text')) + '\n')
        webdriver('POST', r.s.path + '/alert/accept')
        return True
    from Xlib import X, XK
    from Xlib.protocol import event
    d = close_window()
    windows = lambda: _windows(d)
    dialog = wait(lambda: next((w for w, name in windows() if 'Quit Nuzky' in name), None), 5, 0.1)
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
    """The agent's side: `nuzky mcp` attached to the open app over its local socket."""

    def __init__(self, r, project=None, command=None):
        """`command`: what an agent's config runs instead, such as `nuzky-app mcp --current`."""
        command = command or [str(CLI), 'mcp', '--project', str(project), '--allow-write']
        self.process = subprocess.Popen(command, env=r.env,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(r.work / 'bridge.log', 'w'),
                                        text=True, start_new_session=True)
        self.lines, self.id = queue.Queue(), 0
        threading.Thread(target=lambda: [self.lines.put(line) for line in self.process.stdout], daemon=True).start()
        try:
            self.rpc('initialize', {'protocolVersion': '2025-03-26', 'capabilities': {}, 'clientInfo': {'name': 'repro', 'version': '1'}})
            self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        except BaseException:
            stop(self.process)
            raise

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


def comma_locale(r):
    """Starts the app with numbers written with a decimal comma, as on a Czech system, when such a locale is installed;
    the person model once saw nobody under one. Returns the locale, or None."""
    installed = subprocess.run(['locale', '-a'], capture_output=True, text=True).stdout.split()
    found = next((name for name in ('cs_CZ.utf8', 'de_DE.utf8', 'fr_FR.utf8') if name in installed), None)
    if found:
        # LC_ALL would win over it.
        r.env.pop('LC_ALL', None)
        r.env['LC_NUMERIC'] = found
    return found


# The word timing models recognition downloads on first use; linked so no flow reaches the network.
ALIGN_MODELS = ('wav2vec2-xls-r-300m-cs-250-q8_0.gguf', 'wav2vec2-base-960h.gguf')


def link_models(r, names=('ggml-small.bin', 'ggml-silero-v5.1.2.bin')):
    models = r.work / 'data/nuzky/models'
    models.mkdir(parents=True)
    for name in (*names, *ALIGN_MODELS):
        try:
            os.link(MODELS / name, models / name)
        except OSError:
            shutil.copy(MODELS / name, models / name)
