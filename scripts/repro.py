#!/usr/bin/env python3
"""Drives CapOpen flows in the real desktop app and keeps the proof: screenshots, logs and result.json.

    python3 scripts/repro.py --list         flows and what they prove
    python3 scripts/repro.py edit export    run flows; exit code 1 when a check fails
    python3 scripts/repro.py --all

Each flow gets fresh data, cache and runtime directories under tmp-test/repro/<flow>/, so the app never
opens the user's projects or joins their running CapOpen. It runs inside headless gamescope with D-Bus
switched off, so no window, dialog or notification reaches the desktop. Media and models come from
scripts/fixtures.sh.

Needs gamescope, WebKitWebDriver, tauri-driver (`cargo install tauri-driver --locked`), Pillow and
python-xlib. Vite takes port 1420 (the app's dev URL) and tauri-driver 4444. A busy port belongs to
another session, so the run stops instead of touching it.
"""
import base64, datetime, json, os, queue, shutil, signal, socket, subprocess, sys, tempfile, threading, time
import traceback, urllib.error, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / 'tmp-test'
MODELS = FIXTURES / 'xdg/data/capopen/models'
OUT = ROOT / 'tmp-test/repro'
TARGET = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
APP, CLI = TARGET / 'debug/capopen-app', TARGET / 'debug/capopen'
DRIVER = shutil.which('tauri-driver') or str(Path.home() / '.cargo/bin/tauri-driver')
WEBKIT_DRIVER = shutil.which('WebKitWebDriver') or '/usr/bin/WebKitWebDriver'
AI_EDITING = 'AI is editing. Stop it to edit yourself.'
FLOWS = {}


def flow(name, description, before=None):
    def register(fn):
        FLOWS[name] = (description, before, fn)
        return fn
    return register


# --- WebDriver client for the app's webview, through tauri-driver -------------------------------------

def webdriver(method, path, body=None, retries=0):
    """Retries only queries: tauri-driver sometimes reuses a keep-alive connection WebKitWebDriver just closed."""
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
        caps = {'capabilities': {'alwaysMatch': {'browserName': 'wry', 'tauri:options': {'application': str(APP)}}}}
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
        driver = start([DRIVER, '--port', '4444', '--native-driver', WEBKIT_DRIVER], r.work / 'driver.log', r.env)
        if not wait(lambda: port_busy(4444), 20):
            raise RuntimeError('tauri-driver did not start, see driver.log')
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


# --- Flows ------------------------------------------------------------------------------------------------

@flow('edit', 'Import two clips, split one with S, delete a piece with Delete, undo, and find the result saved')
def edit(r):
    r.import_media(FIXTURES / 'talk.mp4', FIXTURES / 'wide.mp4')
    for name in ('talk.mp4', 'wide.mp4'):
        r.add_clip(name)
    clips = r.track()
    r.check('clips sit back to back on the main track', clips[1]['startUs'] == clips[0]['startUs'] + clips[0]['durationUs'], clips)
    r.check('both clips get thumbnails', wait(lambda: len(r.state()['thumbs']) == 2), r.state()['thumbs'])
    r.seek(2_000_000)
    r.focus_clip(clips[0]['id'])
    r.key('s')
    split = wait(lambda: (c := r.track()) and len(c) == 3 and c)
    r.check('S splits the clip at the playhead', split and split[0]['durationUs'] == 2_000_000, split)
    piece = split[1]
    r.focus_clip(piece['id'])
    r.key('Delete')
    deleted = wait(lambda: (c := r.track()) and len(c) == 2 and c)
    end = lambda clips: clips[-1]['startUs'] + clips[-1]['durationUs']
    r.check('Delete removes the piece and closes the gap', deleted and end(deleted) == end(split) - piece['durationUs'], deleted)
    r.key('z', ctrlKey=True)
    undone = wait(lambda: (c := r.track()) and len(c) == 3 and c)
    r.check('Ctrl+Z brings the piece back', undone == split, undone)
    time.sleep(1.5)
    r.shot('timeline')
    # talk.mp4 is portrait and fills the 9:16 canvas; wide.mp4, which followed the deleted piece, leaves it black at the top.
    top = preview_brightness(r, r.work / 'timeline.png', 0.1)
    r.check('the preview shows the restored piece under the playhead', top > 40, round(top, 1))
    saved = lambda: len(json.loads(r.saved_project().read_text())['tracks'][0]['clips'])
    r.check('the project file on disk has the same three clips', wait(lambda: saved() == 3, 10), saved())
    r.check('no error toast', not r.errors(), r.errors())


def preview_rect(r):
    return r.s.run("""const b = document.querySelector('section[aria-label=Preview] canvas').getBoundingClientRect();
        return {left: b.left, top: b.top, width: b.width, height: b.height, viewport: window.innerWidth};""")


def preview_crop(screenshot, rect):
    from PIL import Image
    image = Image.open(screenshot).convert('RGB')
    scale = image.width / rect['viewport']
    left, top = int(rect['left'] * scale), int(rect['top'] * scale)
    return image.crop((left, top, left + int(rect['width'] * scale), top + int(rect['height'] * scale)))


def preview_brightness(r, screenshot, fraction):
    """Mean brightness, 0 to 255, of the top `fraction` of the preview."""
    from PIL import ImageStat
    crop = preview_crop(screenshot, preview_rect(r))
    band = crop.crop((0, 0, crop.width, max(1, int(crop.height * fraction)))).convert('L')
    return ImageStat.Stat(band).mean[0]


# Where the yellow marker of the stored top-left corner must appear on screen, per EXIF orientation.
ORIENTATIONS = {2: 'top-right', 4: 'bottom-left', 6: 'top-right', 7: 'bottom-right'}


def marker(screenshot, rect):
    """Corner of the yellow marker inside the photo, and whether the photo shows as portrait."""
    crop = preview_crop(screenshot, rect)
    yellow, photo = [], []
    pixels = crop.tobytes()
    for i in range(crop.width * crop.height):
        red, green, blue = pixels[3 * i:3 * i + 3]
        point = (i % crop.width, i // crop.width)
        if red > 180 and green > 140 and blue < 100:
            yellow.append(point)
            photo.append(point)
        elif blue > 140 and red < 110:
            photo.append(point)
    if not yellow or not photo:
        return None, None
    xs, ys = [p[0] for p in photo], [p[1] for p in photo]
    cx, cy = sum(p[0] for p in yellow) / len(yellow), sum(p[1] for p in yellow) / len(yellow)
    corner = ('top' if cy < (min(ys) + max(ys)) / 2 else 'bottom') + '-' + ('left' if cx < (min(xs) + max(xs)) / 2 else 'right')
    return corner, 'portrait' if max(ys) - min(ys) > max(xs) - min(xs) else 'landscape'


@flow('orientation', 'Phone photos with EXIF orientation 2, 4, 6 and 7 show the right way round in the preview')
def orientation(r):
    from PIL import Image
    photos = []
    for number in ORIENTATIONS:
        image = Image.new('RGB', (400, 200), (40, 90, 200))
        image.paste((240, 200, 30), (0, 0, 120, 60))
        exif = Image.Exif()
        exif[0x0112] = number
        photos.append(r.work / f'orientation-{number}.jpg')
        image.save(photos[-1], exif=exif.tobytes())
    r.import_media(*photos)
    for photo in photos:
        r.add_clip(photo.name)
    names = {a['id']: a['name'] for a in r.state()['assets']}
    rect = preview_rect(r)
    for clip in r.track():
        number = int(names[clip['assetId']].split('-')[1].split('.')[0])
        r.seek(clip['startUs'] + clip['durationUs'] // 2)
        time.sleep(1.5)
        r.shot(f'orientation-{number}')
        corner, shape = marker(r.work / f'orientation-{number}.png', rect)
        r.check(f'EXIF {number}: the marked corner shows {ORIENTATIONS[number]}', corner == ORIENTATIONS[number], corner)
        expected = 'portrait' if number >= 5 else 'landscape'
        r.check(f'EXIF {number}: the photo shows as {expected}', shape == expected, shape)


def ffprobe(path):
    out = subprocess.run(['ffprobe', '-v', 'error', '-show_entries', 'format=duration:stream=codec_name', '-of', 'json', path],
                         capture_output=True, text=True, check=True).stdout
    data = json.loads(out)
    return float(data['format']['duration']), {s['codec_name'] for s in data['streams']}


def changed_share(a, b):
    """Share of pixels whose brightness clearly differs. Encoding noise stays below the threshold;
    the neighbouring frame of the test pattern changes about 1 % of the picture."""
    from PIL import Image, ImageChops, ImageStat
    first, second = Image.open(a).convert('L'), Image.open(b).convert('L')
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


@flow('export', 'Export refuses to overwrite, replaces only when asked, matches the rendered frame, and quitting mid-export cleans up')
def export(r):
    r.import_media(FIXTURES / 'talk.mp4')
    r.add_clip('talk.mp4')
    epoch = r.state()['epoch']
    target = r.work / 'existing.mp4'
    target.write_bytes(b'not a video')
    start_export = 'window.__capopen.api.startExport(arguments[0], arguments[1], arguments[2], arguments[3])'
    options = {'resolution': 720, 'fps': 30, 'quality': 'small'}
    refused = r.s.call(start_export, str(target), options, epoch, False)
    r.check('an existing file is not replaced without asking', not refused['ok'] and target.read_bytes() == b'not a video', refused)
    started = r.s.call(start_export, str(target), options, epoch, True)
    r.check('export starts once replacing is confirmed', started['ok'], started)
    job = wait(lambda: next((j for j in r.state()['jobs'] if j['kind'] == 'export' and j['status'] != 'running'), None), 180)
    r.check('export finishes', job and job['status'] == 'done', job)
    duration, codecs = ffprobe(target)
    timeline = r.track()[0]['durationUs'] / 1e6
    r.check('the file has H.264 video and AAC sound', codecs == {'h264', 'aac'}, codecs)
    r.check('the file lasts as long as the timeline', abs(duration - timeline) < 0.1, [duration, timeline])
    # The preview renderer draws this frame from the saved project; the export must show the same picture.
    engine, exported = r.work / 'frame-engine.png', r.work / 'frame-export.png'
    subprocess.run([str(CLI), 'frame', str(r.saved_project()), '5', str(engine), '720'], env=r.env, check=True,
                   capture_output=True)
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', str(target), '-vf', 'select=eq(n\\,150)', '-frames:v', '1',
                    '-update', '1', str(exported)], check=True)
    changed = changed_share(engine, exported)
    r.check('exported frame 150 (5 s at 30 fps) matches the rendered one', changed < 0.002, f'{changed:.3%} of pixels differ')
    r.check('no unfinished file is left', not list(r.work.glob('.capopen-part-*')))
    # Closing the window during an export asks first, then removes the unfinished file.
    quit_target = r.work / 'quit.mp4'
    started = r.s.call(start_export, str(quit_target), {'resolution': 2160, 'fps': 60, 'quality': 'high'}, epoch, True)
    r.check('a long export runs into an unfinished file', started['ok'] and wait(lambda: list(r.work.glob('.capopen-part-*')), 10))
    r.check('closing the window asks before quitting', close_window_and_confirm(r))
    gone = wait(lambda: not list(r.work.glob('.capopen-part-*')) and not quit_target.exists(), 15)
    r.check('quitting removes the unfinished export', gone, [p.name for p in r.work.glob('.capopen-part-*')])


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


def agent_project(r):
    project = r.work / 'data/capopen/projects/agent.capopen'
    subprocess.run([str(CLI), 'new', str(project), str(FIXTURES / 'talk.mp4')], env=r.env, check=True, capture_output=True)


@flow('agent', 'An AI agent edits the open project live: the UI locks with a reason, the edit appears, one undo removes the run',
      before=agent_project)
def agent(r):
    clip = r.track()[0]
    r.check('the app opened the project the agent will edit', len(r.track()) == 1)
    bridge = Bridge(r, r.saved_project())
    try:
        run = bridge.call('begin_run', {'label': 'Tighten the intro'})
        r.check('the UI knows an agent is editing', wait(lambda: r.state()['aiRun'], 10))
        button = r.s.run("""const b = [...document.querySelectorAll('button')].find((b) => b.textContent.includes('Import media'));
            return b && {disabled: b.getAttribute('aria-disabled'), reason: b.title};""")
        r.check('controls are locked and say why', button and button['disabled'] == 'true' and button['reason'], button)
        r.focus_clip(clip['id'])
        for _ in range(2):
            r.key('Delete')
            time.sleep(0.5)
        notices = [t for t in r.state()['toasts'] if t['text'] == AI_EDITING]
        r.check('a user edit during the run is refused with one notice', len(r.track()) == 1 and len(notices) == 1, r.state()['toasts'])
        r.shot('locked')
        edit = {'type': 'splitClip', 'clipId': clip['id'], 'atUs': 3_000_000}
        bridge.call('apply_edits', {'run_id': run['run_id'], 'request_id': 'split', 'edits': [edit]})
        r.check("the agent's edit appears live", wait(lambda: len(r.track()) == 2, 10), r.track())
        bridge.call('end_run', {'run_id': run['run_id'], 'action': 'keep'})
        r.check('the lock ends with the run', wait(lambda: not r.state()['aiRun'], 10))
        r.shot('after-run')
        r.s.run('document.activeElement?.blur()')
        r.key('z', ctrlKey=True)
        r.check('one undo removes the whole run', wait(lambda: len(r.track()) == 1, 10), r.track())
        r.check('no error toast', not r.errors(), r.errors())
    finally:
        bridge.close()


def link_models(r):
    models = r.work / 'data/capopen/models'
    models.mkdir(parents=True)
    for name in ('ggml-small.bin', 'ggml-silero-v5.1.2.bin'):
        try:
            os.link(MODELS / name, models / name)
        except OSError:
            shutil.copy(MODELS / name, models / name)


@flow('captions', 'Whisper on this computer captions the spoken words of a video', before=link_models)
def captions(r):
    r.import_media(FIXTURES / 'talk.mp4')
    r.add_clip('talk.mp4')
    r.s.run("window.__capopen.speech.setState({model: 'small', language: 'en'})")
    r.s.run("[...document.querySelectorAll('[role=tab]')].find((t) => t.textContent.trim() === 'Captions').click()")
    time.sleep(0.5)
    r.s.run("[...document.querySelectorAll('button')].find((b) => b.textContent.includes('Generate captions')).click()")
    job = wait(lambda: next((j for j in r.state()['jobs'] if j['kind'] == 'captions' and j['status'] != 'running'), None), 300, 1)
    r.check('captions finish', job and job['status'] == 'done', job)
    texts = [c['text'] for t in r.state()['tracks'] if t['kind'] == 'text' for c in t['clips'] if c['text']]
    heard = ' '.join(texts).lower()
    spoken = ['channel', 'video', 'laptop', 'clips', 'pauses', 'captions']
    found = [w for w in spoken if w in heard]
    r.check('captions contain the spoken words', len(found) >= 4, {'found': found, 'captions': texts[:8]})
    r.seek(2_000_000)
    time.sleep(1.5)
    r.shot('captions')
    r.check('no error toast', not r.errors(), r.errors())


# --- Entry point ------------------------------------------------------------------------------------------

def preflight(names):
    problems = [f'{tool} is missing' for tool in ('gamescope', 'ffmpeg', 'npx') if not shutil.which(tool)]
    problems += [f'{path} is missing' for path in (DRIVER, WEBKIT_DRIVER) if not Path(path).exists()]
    for module, package in (('PIL', 'Pillow'), ('Xlib', 'python-xlib')):
        try:
            __import__(module)
        except ImportError:
            problems.append(f'Python package {package} is missing')
    if not (FIXTURES / 'talk.mp4').exists() or ('captions' in names and not (MODELS / 'ggml-small.bin').exists()):
        problems.append('test media or models are missing: run scripts/fixtures.sh')
    problems += [f'port {port} is in use by another session' for port in (1420, 4444) if port_busy(port)]
    return problems


def main(args):
    if not args or args == ['--list']:
        print('Flows (python3 scripts/repro.py <flow>... | --all):\n')
        for name, (description, _, _) in FLOWS.items():
            print(f'  {name:<12} {description}')
        return 0
    names = list(FLOWS) if args == ['--all'] else args
    unknown = [n for n in names if n not in FLOWS]
    if unknown:
        print(f'Unknown flow: {", ".join(unknown)}. Run python3 scripts/repro.py --list.', file=sys.stderr)
        return 2
    if os.environ.get('CAPOPEN_REPRO_INNER') == '1':
        OUT.mkdir(parents=True, exist_ok=True)
        vite = start(['npx', 'vite', '--port', '1420', '--strictPort'], OUT / 'vite.log')
        try:
            if not wait(lambda: port_busy(1420), 30):
                print('Vite did not start, see tmp-test/repro/vite.log', file=sys.stderr)
                return 1
            for name in names:
                run_flow(name)
        finally:
            stop(vite)
        return 0
    problems = preflight(names)
    if problems:
        print('repro cannot run:\n  ' + '\n  '.join(problems), file=sys.stderr)
        return 1
    subprocess.run(['cargo', 'build', '--locked', '-p', 'capopen-app', '-p', 'capopen-cli'], cwd=ROOT, check=True)
    for name in names:
        (OUT / name / 'result.json').unlink(missing_ok=True)
    OUT.mkdir(parents=True, exist_ok=True)
    print(f'Running {", ".join(names)} in headless gamescope; its output goes to tmp-test/repro/gamescope.log', flush=True)
    with open(OUT / 'gamescope.log', 'w') as log:
        subprocess.run(['gamescope', '--backend', 'headless', '-W', '1440', '-H', '900', '--', sys.executable, __file__, *names],
                       cwd=ROOT, env=dict(os.environ, CAPOPEN_REPRO_INNER='1'), stdout=log, stderr=subprocess.STDOUT)
    failed = []
    for name in names:
        result_file = OUT / name / 'result.json'
        result = json.loads(result_file.read_text()) if result_file.exists() else None
        print(f"\n{'passed' if result and result['passed'] else 'FAILED'}  {name}  tmp-test/repro/{name}/")
        for check in result['checks'] if result else []:
            detail = '' if check['ok'] or check['detail'] is None else f": {check['detail']}"
            print(f"  {'ok  ' if check['ok'] else 'FAIL'} {check['check']}{detail}")
        if not result or result['error']:
            print('  ' + (result['error'] if result else 'no result; see tmp-test/repro/gamescope.log').strip().replace('\n', '\n  '))
        failed += [] if result and result['passed'] else [name]
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
