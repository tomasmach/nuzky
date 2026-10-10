"""Picture in picture over a talking head. One click on a media tile puts a 16:9 video in a rounded window in the
top right corner of the safe area; the inspector turns it into a circle with a border, typed with the keyboard;
a bar in the preview crops it; and the export shows the same picture as the preview, with the talking head in the
corners of the circle's square."""
import json, subprocess, time

from PIL import Image, ImageStat

from e2e.harness import CLI, FIXTURES, changed_share, flow, press, preview_crop, preview_rect, preview_redraw, wait, webdriver

AT_US = 1_000_000
PROJECT = 'return window.__nuzky.store.getState().snap.project'
SECTION = "aside[aria-label=Inspector]"


def select(r, ids):
    """The selection box would show in the preview shots; the inspector needs the clip selected."""
    r.s.run('window.__nuzky.store.getState().select(arguments[0])', ids)


def project(r):
    return r.s.run(PROJECT, retries=3)


def pip_clip(r, asset_id):
    """(track index, clip) of the picture in picture."""
    return next(((i, c) for i, t in enumerate(project(r)['tracks']) for c in t['clips'] if c['content'].get('assetId') == asset_id),
                (None, None))


def bounds(r, clip_id):
    found = r.s.call(f'window.__nuzky.api.layerBounds({AT_US}).then((b) => JSON.stringify(b))')
    return next((b for b in json.loads(found['value']) if b['clipId'] == clip_id), None) if found['ok'] else None


def box(corners, scale):
    xs, ys = [p[0] * scale for p in corners], [p[1] * scale for p in corners]
    return min(xs), min(ys), max(xs), max(ys)


def region_change(a, b, area):
    return changed_share(a.crop(tuple(round(v) for v in area)), b.crop(tuple(round(v) for v in area)))


def engine_frame(r, name):
    path = r.work / f'{name}.png'
    subprocess.run([str(CLI), 'frame', str(r.saved_project()), f'{AT_US / 1e6:.6f}', str(path), '720'], env=r.env, check=True,
                   capture_output=True)
    return path


def click_button(r, text, scope=SECTION):
    return r.s.run("const b = [...document.querySelector(arguments[1]).querySelectorAll('button')].find((b) => b.textContent.trim() === arguments[0]);"
                   "if (!b) return false; b.click(); return true;", text, scope)


@flow('pip', 'A video goes into a rounded corner window over a talking head in one click; it becomes a circle with a '
      'border, a preview handle crops it, and the export matches the preview')
def pip(r):
    r.import_media(FIXTURES / 'talk.mp4', FIXTURES / 'wide.mp4')
    r.add_clip('talk.mp4')
    wide = next(a['id'] for a in r.state()['assets'] if a['name'] == 'wide.mp4')
    canvas = project(r)['canvas']
    r.seek(AT_US)
    select(r, [])
    rect = preview_rect(r)
    # Until the frame at the playhead arrives, the preview shows the first one, which every later shot would differ from.
    drawn = lambda: r.s.run("return document.querySelector('section[aria-label=Preview] canvas').dataset.us")
    if not r.check('the preview draws the frame at the playhead', wait(lambda: drawn() == str(AT_US), 10), drawn()):
        return

    def talking_head():
        r.shot('talking-head')
        crop = preview_crop(r.work / 'talking-head.png', rect)
        return crop if ImageStat.Stat(crop.convert('L')).mean[0] > 40 else None
    before = wait(talking_head, 10, 0.5)
    if not r.check('the preview shows the talking head', before):
        return
    time.sleep(1.5)  # `nuzky frame` reads the saved file, written about a second after an edit.
    engine_before = engine_frame(r, 'engine-talking-head')

    # The tile's action, as a click on the button that shows on hover or focus.
    tile = "document.querySelector('[aria-label=\"Add wide.mp4 as picture in picture\"]')"
    if not r.check('a video tile offers picture in picture', r.s.run(f'return !!{tile}')):
        return
    r.s.run(f'{tile}.click()')
    index, clip = wait(lambda: (found := pip_clip(r, wide))[1] and found, 10) or (None, None)
    if not r.check('the video lands on a track right above the main track', index == 1, index):
        return
    r.check('it starts at the playhead and plays the whole video', (clip['startUs'], clip['durationUs']) == (AT_US, 5_000_000),
            (clip['startUs'], clip['durationUs']))
    r.check('the new clip is selected', r.s.run('return window.__nuzky.store.getState().selection') == [clip['id']])
    shape = clip['content'].get('shape') or {}
    r.check('it has rounded corners and a soft shadow', abs(shape.get('radius', 0) - 0.15) < 1e-6 and shape.get('shadow') == 0.5, shape)
    placed = bounds(r, clip['id'])
    area = canvas['safeArea']
    r.check('its top right corner is the top right corner of the safe area',
            placed and abs(placed['corners'][1][0] - area['right']) < 0.5 and abs(placed['corners'][1][1] - area['top']) < 0.5,
            placed and placed['corners'][1])
    wait(lambda: r.s.run('return !!document.querySelector(\'[data-handle="rotate"]\')'), 5)
    time.sleep(1)
    r.shot('pip-added-selected')
    select(r, [])
    after, redrawn = preview_redraw(r, 'pip-added', before)
    scale = after.width / canvas['width']
    window = box(placed['corners'], scale)
    left, top, right, bottom = window
    inside = region_change(before, after, (left + 6, top + 6, right - 6, bottom - 6))
    below = region_change(before, after, (0, bottom + 20, after.width, after.height))
    r.check('the preview shows the video in that window and the talking head everywhere else',
            redrawn and inside > 0.3 and below < 0.005, {'inside': round(inside, 3), 'below': round(below, 4)})

    r.key('z', ctrlKey=True)
    r.check('one undo removes the window and its track', wait(lambda: pip_clip(r, wide)[1] is None and len(project(r)['tracks']) == 1, 5))
    r.key('z', ctrlKey=True, shiftKey=True)
    r.check('redo brings it back', wait(lambda: pip_clip(r, wide)[1] is not None, 5))
    select(r, [clip['id']])

    # The inspector: Circle crops the 16:9 video to a square; the border is typed with the keyboard.
    if not r.check('the inspector has a Crop and shape section', wait(lambda: r.s.run(
            f"return [...document.querySelectorAll('{SECTION} h3')].some((h) => h.textContent === 'Crop and shape')"), 5)):
        r.shot('inspector-missing')
        return
    click_button(r, 'Circle')
    crop = lambda: (pip_clip(r, wide)[1]['content']['transform'].get('crop') or {})
    squared = wait(lambda: abs(crop().get('left', 0) - 0.21875) < 1e-3 and crop(), 5)
    r.check('Circle crops the longer sides to a square', squared and abs(squared['right'] - 0.21875) < 1e-3, crop())
    r.check('Circle rounds the corners fully', pip_clip(r, wide)[1]['content']['shape']['radius'] == 1)
    kept = wait(lambda: (b := bounds(r, clip['id'])) and abs(b['corners'][1][0] - area['right']) < 0.5 and b, 5)
    r.check('the circle stays in the corner of the safe area', kept, kept or bounds(r, clip['id']))
    r.s.run(f"document.querySelector('{SECTION} input[aria-label=\"Border\"]').focus()")
    for _ in range(6):
        press('Right')
    border = wait(lambda: (pip_clip(r, wide)[1]['content']['shape']['borderWidth'] == 6) or None, 5)
    r.check('the arrow keys on Border widen it to 6 px', border, pip_clip(r, wide)[1]['content']['shape'])
    r.shot('inspector-circle')

    # The preview: the talking head shows again in the corners of the circle's square.
    select(r, [])
    time.sleep(1.5)
    r.shot('pip-circle')
    circle = preview_crop(r.work / 'pip-circle.png', rect)
    square = bounds(r, clip['id'])
    left, top, right, bottom = box(square['corners'], scale)
    corner = region_change(before, circle, (left, top, left + 4, top + 4)) + region_change(before, circle, (right - 4, bottom - 4, right, bottom))
    middle = region_change(before, circle, ((left + right) / 2 - 8, (top + bottom) / 2 - 8, (left + right) / 2 + 8, (top + bottom) / 2 + 8))
    r.check('the preview draws a circle: the talking head in the corners, the video in the middle',
            corner < 0.01 and middle > 0.3, {'corners': round(corner, 3), 'middle': round(middle, 3)})

    # A crop handle: dragging the top bar down to the middle crops the top half away.
    select(r, [clip['id']])
    wait(lambda: r.s.run('return !!document.querySelector(\'[data-handle="crop-top"]\')'), 5)
    drag = """const h = document.querySelector('[data-handle="crop-top"]'); if (!h) return false;
    const b = h.getBoundingClientRect(); const x = b.left + b.width / 2;
    const at = (type, y) => new PointerEvent(type, {bubbles: true, button: 0, clientX: x, clientY: y, pointerId: 1});
    h.dispatchEvent(at('pointerdown', b.top + b.height / 2));
    window.dispatchEvent(at('pointermove', arguments[0]));
    window.dispatchEvent(at('pointerup', arguments[0]));
    return true;"""
    middle_y = rect['top'] + (square['frame'][0][1] + square['frame'][3][1]) / 2 * rect['height'] / canvas['height']
    r.check('the selected video has a crop bar on its top edge', r.s.run(drag, middle_y))
    cropped = wait(lambda: abs(crop().get('top', 0) - 0.5) < 0.02 and crop(), 5)
    r.check('dragging the top bar to the middle crops the top half', cropped, crop())
    shown = wait(lambda: (b := bounds(r, clip['id'])) and abs(b['corners'][0][1] - (b['frame'][0][1] + b['frame'][3][1]) / 2) < 2 and b, 5)
    r.check('the selection box follows the crop and the picture stays put',
            shown and shown['frame'] == square['frame'], shown and {'corners': shown['corners'], 'frame': shown['frame']})
    r.shot('pip-cropped')
    r.key('z', ctrlKey=True)
    r.check('one undo takes the drag back', wait(lambda: abs(crop().get('top', 1) - 0) < 1e-6, 5), crop())

    # The export shows what the preview shows.
    time.sleep(1.5)
    engine = engine_frame(r, 'engine-circle')
    target = r.work / 'pip.mp4'
    started = r.s.call('window.__nuzky.api.startExport(arguments[0], arguments[1], arguments[2], arguments[3])', str(target),
                       {'resolution': 720, 'fps': 30, 'quality': 'recommended'}, r.state()['epoch'], False)
    job = started['ok'] and wait(lambda: next((j for j in r.state()['jobs'] if j['kind'] == 'export' and j['status'] != 'running'), None), 180)
    if not r.check('the export finishes', job and job['status'] == 'done', job or started):
        return
    exported = r.work / 'export-circle.png'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', str(target), '-vf', f'select=eq(n\\,{AT_US * 30 // 1_000_000})',
                    '-frames:v', '1', '-update', '1', str(exported)], check=True)
    r.shots.append(exported.name)
    changed = changed_share(engine, exported)
    r.check('the exported frame matches the frame the renderer draws for the preview', changed < 0.002, f'{changed:.3%} of pixels differ')
    out, plain = Image.open(exported).convert('RGB'), Image.open(engine_before).convert('RGB')
    k = out.width / canvas['width']
    left, top, right, bottom = box(square['corners'], k)
    corner = region_change(plain, out, (left + 1, top + 1, left + 4, top + 4))
    middle = region_change(plain, out, ((left + right) / 2 - 6, (top + bottom) / 2 - 6, (left + right) / 2 + 6, (top + bottom) / 2 + 6))
    r.check('the export draws the circle: the talking head in the corners of its square, the video inside',
            corner < 0.01 and middle > 0.3, {'corners': round(corner, 3), 'middle': round(middle, 3)})
    r.check('no error toast', not r.errors(), r.errors())

    # Proof of the look: the tile's actions and the section in the largest and the smallest window.
    select(r, [clip['id']])
    show = ("[...document.querySelectorAll('aside[aria-label=Inspector] h3')].find((h) => h.textContent === 'Crop and shape')"
            ".closest('section').scrollIntoView({block: 'end'})")
    for width, height, name in ((1440, 900, 'look'), (1024, 640, 'look-1024')):
        webdriver('POST', r.s.path + '/window/rect', {'width': width, 'height': height})
        wait(lambda: r.s.run('return window.innerWidth') == width, 5)
        r.s.run(f'{tile}.focus()')
        r.s.run(show)
        time.sleep(1)
        r.shot(name)
