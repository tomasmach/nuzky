"""A background behind the person in a phone talking head: Blur, turned on by keyboard in the inspector, finds the
person in the background with its progress shown, and the preview and the export blur the room and keep the person
sharp; Color puts the colour behind them in the export with steady edges, as the renderer draws it; one undo takes it
back; turning it off stops the work; an agent attached to the app puts an image behind the person over MCP."""
import json, os, shutil, subprocess, time

import numpy as np

from e2e.harness import CLI, FIXTURES, MODELS, Bridge, changed_share, comma_locale, export, flow, press, preview_crop, preview_rect, wait, webdriver

SOURCE = FIXTURES / 'talking-head.mov'
JOB = "return window.__nuzky.store.getState().jobs[arguments[0]] ?? null"
CLIP = "return window.__nuzky.store.getState().snap.project.tracks[0].clips[0]"
ASSETS = "return window.__nuzky.store.getState().snap.project.assets"
SECTION = "aside[aria-label=Inspector]"
SIZE = (360, 640)
# At 360 x 640: the room above the shoulders on both sides, and the middle of the face, clear of the edge as she moves.
ROOM = [(slice(10, 190), slice(6, 60)), (slice(10, 140), slice(300, 354))]
FACE = (slice(100, 190), slice(140, 220))


def frames(path):
    """Every frame of a video as RGB at 360 x 640."""
    w, h = SIZE
    raw = subprocess.run(['ffmpeg', '-v', 'error', '-i', str(path), '-vf', f'scale={w}:{h}', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'],
                         capture_output=True, check=True).stdout
    return np.frombuffer(raw, np.uint8).reshape(-1, h, w, 3).astype(np.float32)


def sharpness(picture, area):
    """Variance of the Laplacian of the luma: high where there is fine detail."""
    g = (picture[area] @ [0.2126, 0.7152, 0.0722])
    lap = 4 * g[1:-1, 1:-1] - g[:-2, 1:-1] - g[2:, 1:-1] - g[1:-1, :-2] - g[1:-1, 2:]
    return float(lap.var())


def green(picture):
    return (picture[..., 1] > 200) & (picture[..., 0] < 80) & (picture[..., 2] < 80)


def frame_of(screenshot, rect):
    """The 9:16 frame the preview draws in the middle of its canvas, at 360 x 640."""
    crop = preview_crop(screenshot, rect)
    w = min(crop.width, crop.height * 9 / 16)
    h = w * 16 / 9
    left, top = (crop.width - w) / 2, (crop.height - h) / 2
    return np.asarray(crop.crop((round(left), round(top), round(left + w), round(top + h))).resize(SIZE), np.float32)


def look(r, name):
    """The Background section in view, at 1440 x 900 and at the smallest window, 1024 x 640."""
    show = ("[...document.querySelectorAll('aside[aria-label=Inspector] h3')].find((h) => h.textContent === 'Background')"
            ".closest('section').scrollIntoView({block: 'center'})")
    for width, height, suffix in ((1024, 640, '-1024'), (1440, 900, '')):
        webdriver('POST', r.s.path + '/window/rect', {'width': width, 'height': height})
        wait(lambda: r.s.run('return window.innerWidth') == width, 5)
        r.s.run(show)
        time.sleep(1)
        r.shot(name + suffix)


def select(r, ids):
    r.s.run('window.__nuzky.store.getState().select(arguments[0])', ids)


def link_person_model(r):
    comma_locale(r)
    models = r.work / 'data/nuzky/models'
    models.mkdir(parents=True, exist_ok=True)
    try:
        os.link(MODELS / 'selfie-segmenter.onnx', models / 'selfie-segmenter.onnx')
    except OSError:
        shutil.copy(MODELS / 'selfie-segmenter.onnx', models / 'selfie-segmenter.onnx')


def set_background(r, clip_id, background):
    return r.s.call("window.__nuzky.store.getState().edit({type: 'updateClip', clipId: arguments[0], background: arguments[1]})",
                    clip_id, background)


def finished(r, job_id, timeout=120):
    return wait(lambda: (j := r.s.run(JOB, job_id)) and j['status'] != 'running' and j, timeout, 0.1)


@flow('background', 'Blur and replace the background behind a person in a phone talking head, in the preview, the '
      'export and over MCP, with the person found in the background and edges that hold still', before=link_person_model)
def background(r):
    if not SOURCE.exists():
        raise RuntimeError('tmp-test/talking-head.mov is missing: run scripts/fixtures.sh')
    r.import_media(SOURCE)
    r.add_clip('talking-head.mov')
    clip = r.track()[0]
    path = next(a['path'] for a in r.s.run(ASSETS) if a['id'] == clip['assetId'])
    plain = frames(export(r, 'plain.mp4'))
    r.seek(4_000_000)
    if not wait(lambda: r.s.run('return window.__nuzky.store.getState().timeUs') == 4_000_000, 10):
        raise RuntimeError('the playhead did not move to 4 s')
    # The paused preview waits for the exact frame there; the selection box would show in the shots.
    select(r, [])
    time.sleep(1.5)
    rect = preview_rect(r)
    r.shot('preview-plain')
    before = frame_of(r.work / 'preview-plain.png', rect)

    # Keyboard: the Background group in the Video tab, Tab from None onto Blur, then Enter (Space plays).
    r.focus_clip(clip['id'])
    none = f"document.querySelector('{SECTION} [role=group][aria-label=Background] button')"
    if not wait(lambda: r.s.run(f'return !!{none}'), 10):
        raise RuntimeError('the inspector shows no Background section for the video')
    r.s.run(f'{none}.focus()')
    press('Tab')
    focused = wait(lambda: r.s.run('return document.activeElement.textContent') == 'Blur', 3, 0.05)
    r.check('Tab from None reaches Blur', focused, r.s.run('return document.activeElement.outerHTML.slice(0, 160)'))
    press('Return')
    kind = lambda: (r.s.run(CLIP)['content'].get('background') or {}).get('type')
    r.check('Enter turns on a blurred background', wait(lambda: kind() == 'blur', 5), kind())
    job = finished(r, f'matte:{path}')
    r.check('the person is found in the background', job and job['status'] == 'done', job)
    saved = lambda: json.loads(r.saved_project().read_text())['tracks'][0]['clips'][0]['content'].get('background')
    r.check('the project file on disk has the background', wait(lambda: saved() == {'type': 'blur', 'strength': 0.5}, 10), saved())
    r.check('the progress row goes once the person is found',
            wait(lambda: not r.s.run(f"return [...document.querySelectorAll('{SECTION} [role=status]')].some((e) => e.textContent.startsWith('Finding'))"), 5))

    # The preview blurs the room behind her, even paused, and keeps her face as it was.
    select(r, [])
    time.sleep(1)
    r.shot('preview-blur')
    after = frame_of(r.work / 'preview-blur.png', rect)
    from PIL import Image
    Image.fromarray(np.hstack([before, after]).astype(np.uint8)).save(r.work / 'preview-before-after.png')
    r.shots.append('preview-before-after.png')
    room = [round(sharpness(after, a) / max(sharpness(before, a), 1e-3), 3) for a in ROOM]
    face = round(sharpness(after, FACE) / max(sharpness(before, FACE), 1e-3), 3)
    r.check('the paused preview blurs the room and keeps the face sharp', max(room) < 0.4 and face > 0.75, {'room': room, 'face': face, 'rect': rect})

    blurred = frames(export(r, 'blur.mp4'))
    ratios = {'room': [], 'face': []}
    for k in (30, 120, 210):
        ratios['room'] += [sharpness(blurred[k], a) / max(sharpness(plain[k], a), 1e-3) for a in ROOM]
        ratios['face'].append(sharpness(blurred[k], FACE) / max(sharpness(plain[k], FACE), 1e-3))
    r.check('the export blurs the room behind her and keeps her face sharp', max(ratios['room']) < 0.4 and min(ratios['face']) > 0.8,
            {k: [round(v, 3) for v in vs] for k, vs in ratios.items()})

    select(r, [clip['id']])
    # The progress row while the person is found. An 8 s clip is done before a screenshot lands, so the job event
    # of a running job is replayed into the store.
    row = (f"return [...document.querySelectorAll('{SECTION} [role=status]')].map((e) => e.textContent)"
           ".find((t) => t.startsWith('Finding')) ?? null")
    replay = ("const st = window.__nuzky.store, id = arguments[0], job = st.getState().jobs[id];"
              "st.setState({jobs: {...st.getState().jobs, [id]: {...job, status: arguments[1], progress: arguments[2]}}});")
    r.s.run(replay, f'matte:{path}', 'running', 0.42)
    r.check('a running job shows its progress under the controls', wait(lambda: r.s.run(row) == 'Finding the person · 42%', 5), r.s.run(row))
    look(r, 'inspector-finding')
    r.s.run(replay, f'matte:{path}', 'done', 1)
    look(r, 'inspector-blur')

    # Green behind her: the corners are green in every frame, her face never, and the edge holds still where the
    # picture does.
    set_background(r, clip['id'], {'type': 'color', 'color': '#00ff00'})
    r.check('Color takes the picked colour', wait(lambda: kind() == 'color', 5), kind())
    target = export(r, 'green.mp4')
    keyed = frames(target)
    corners = min(float(green(f[a]).mean()) for f in keyed for a in ROOM)
    face_green = max(float(green(f[FACE]).mean()) for f in keyed)
    r.check('every exported frame has green behind her and none on her face', corners > 0.97 and face_green < 0.01,
            {'room green': round(corners, 4), 'face green': round(face_green, 4)})
    luma = plain @ np.array([0.2126, 0.7152, 0.0722], np.float32)
    alpha = 1 - np.clip((keyed[..., 1] - np.maximum(keyed[..., 0], keyed[..., 2])) / 255, 0, 1)
    edge = ((alpha[1:] > 0.06) & (alpha[1:] < 0.94)) | ((alpha[:-1] > 0.06) & (alpha[:-1] < 0.94))
    still = np.abs(luma[1:] - luma[:-1]) < 4
    band = edge & still
    flicker = float(np.abs(alpha[1:] - alpha[:-1])[band].mean())
    r.check('the edge around her holds still where the picture does', flicker < 0.06, {'flicker': round(flicker, 4), 'samples': int(band.sum())})
    engine = r.work / 'engine-green.png'
    subprocess.run([str(CLI), 'frame', str(r.saved_project()), '4.0', str(engine), '720'], env=r.env, check=True, capture_output=True)
    exported = r.work / 'export-green.png'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', str(target), '-vf', 'select=eq(n\\,120)', '-frames:v', '1', '-update', '1',
                    str(exported)], check=True)
    r.shots += [engine.name, exported.name]
    changed = changed_share(engine, exported)
    r.check('the exported frame matches the frame the renderer draws for the preview', changed < 0.005, f'{changed:.3%} of pixels differ')

    r.key('z', ctrlKey=True)
    r.check('Ctrl+Z brings back the blur in one step', wait(lambda: kind() == 'blur', 5), kind())
    r.key('z', ctrlKey=True)
    r.check('Ctrl+Z again shows the picture as recorded', wait(lambda: kind() is None, 5), kind())
    r.check('no error toast', not r.errors(), r.errors())

    over_mcp(r)
    stops_when_turned_off(r)


def over_mcp(r):
    """An agent puts an imported image behind the person in a copy of the video: inspect_frames waits for the person
    to be found as a job, and export_video has the image behind her."""
    copy = r.work / 'talking-head-copy.mov'
    shutil.copy(SOURCE, copy)
    orange = r.work / 'orange.png'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', 'color=c=#ff8800:s=640x360', '-frames:v', '1', str(orange)], check=True)
    bridge = Bridge(r, r.saved_project())
    try:
        run = bridge.call('begin_run', {'label': 'Orange behind the person'})['run_id']
        video, image = bridge.call('import_media', {'run_id': run, 'paths': [str(copy), str(orange)], 'request_id': 'media'})['asset_ids']
        last = r.track()[-1]
        start = last['startUs'] + last['durationUs']
        bridge.call('apply_edits', {'run_id': run, 'request_id': 'add', 'edits': [
            {'type': 'addClip', 'assetId': video, 'startUs': None, 'trackId': None}]})
        added = wait(lambda: next((c for c in r.track() if c['assetId'] == video), None), 10)
        bridge.call('apply_edits', {'run_id': run, 'request_id': 'bg', 'edits': [
            {'type': 'updateClip', 'clipId': added['id'], 'background': {'type': 'image', 'assetId': image}}]})
        at = start + 4_000_000
        first = bridge.rpc('tools/call', {'name': 'inspect_frames', 'arguments': {'times_us': [at]}})
        text = first['result']['content'][0]['text'] if first['result'].get('isError') else ''
        job = text.split('as job ')[1].split(';')[0] if 'MATTE_NOT_READY' in text else None
        r.check('inspect_frames names the job finding the person', job, first['result'])
        if job:
            done = wait(lambda: (s := bridge.call('job', {'job_id': job, 'action': 'get'}))['status'] != 'running' and s, 120, 0.3)
            r.check('the job finds the person', done and done['status'] == 'done', done)
        sheet = bridge.rpc('tools/call', {'name': 'inspect_frames', 'arguments': {'times_us': [at]}})['result']
        r.check('inspect_frames then shows the frame', not sheet.get('isError') and sheet['content'][1]['type'] == 'image', sheet['content'][0])
        bridge.call('end_run', {'run_id': run, 'action': 'keep'})
        out = r.work / 'agent.mp4'
        export_job = bridge.call('export_video', {'path': str(out), 'resolution': 720, 'fps': 30, 'quality': 'small'})
        done = wait(lambda: (s := bridge.call('job', {'job_id': export_job['job_id'], 'action': 'get'}))['status'] != 'running' and s, 180, 0.5)
        r.check('export_video finishes', done and done['status'] == 'done', done)
        picture = frames(out)[round(at / 1e6 * 30)]
        orange_share = min(float(((np.abs(picture[a] - [255, 136, 0]) < 40).all(-1)).mean()) for a in ROOM)
        r.check('the export has the image behind her', orange_share > 0.97, round(orange_share, 4))
        select(r, [added['id']])
        r.seek(at)
        look(r, 'inspector-image')
    finally:
        bridge.close()


def stops_when_turned_off(r):
    """Choosing None while the person is found in a long video stops the work within seconds and leaves no
    unfinished file."""
    long = r.work / 'long.mp4'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=s=540x960:r=30:d=240', '-c:v', 'libx264',
                    '-preset', 'ultrafast', '-pix_fmt', 'yuv420p', str(long)], check=True)
    r.import_media(long)
    asset = next(a for a in r.s.run(ASSETS) if a['name'] == 'long.mp4')
    added = r.s.call("window.__nuzky.store.getState().edit({type: 'addClip', assetId: arguments[0], startUs: 0, trackId: null})", asset['id'])
    clip_of = ("return window.__nuzky.store.getState().snap.project.tracks.flatMap((t) => t.clips)"
               ".find((c) => c.content.assetId === arguments[0])?.id ?? null")
    clip = wait(lambda: r.s.run(clip_of, asset['id']), 10)
    if not (added['ok'] and clip):
        raise RuntimeError(f'the long video was not placed: {added}')
    set_background(r, clip, {'type': 'blur', 'strength': 0.5})
    running = wait(lambda: (j := r.s.run(JOB, f"matte:{asset['path']}")) and j['status'] == 'running', 10, 0.05)
    time.sleep(1)
    set_background(r, clip, {'type': 'none'})
    started = time.time()
    job = finished(r, f"matte:{asset['path']}", 10)
    seconds = time.time() - started
    r.check('choosing None stops finding the person in a 4-minute video within 2 s',
            running and job and job['status'] == 'cancelled' and seconds < 2, f"{job and job['status']} after {seconds:.2f} s")
    left = [p.name for p in (r.work / 'cache/nuzky/matte').glob('.*.part')]
    r.check('a stopped job leaves no unfinished file', not left, left)
