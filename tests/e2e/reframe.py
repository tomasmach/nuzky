"""Reframe a wide talking head to 9:16 from the Ratio menu. The fixture is the NASA interview at 1920x1080 with her
face a third of the way across until 5 s, crossing to two thirds by 6 s; the clip is cut at 7 s. Reframe, reached by
keyboard beside 9:16, changes the canvas and writes keyframes that hold still, follow the face once and jump at the
cut, so at every quarter second her face sits whole inside the frame and inside what Reels leaves free. The preview
shows the result with Undo, which brings back the wide canvas and the clips as they were. The face model is linked
from tmp-test, so nothing is downloaded."""
import time

from e2e.harness import FIXTURES, flow, link_models, press, wait
from e2e.home import resize

RATIO = 'section[aria-label=Preview] button[aria-haspopup=menu]'
PROJECT = 'return window.__nuzky.store.getState().snap.project'
# The fixture's face, in fractions of the picture: centre across before and after its move, and its width.
FACE_BEFORE, FACE_AFTER, FACE_WIDTH = 1 / 3, 2 / 3, 0.17
# What Reels and TikTok leave free across a 1080 px wide vertical canvas.
SAFE = (60, 900)


def reframe_setup(r):
    link_models(r, names=('yunet-2026may.onnx',))


def face_across(t_us):
    """The fixture's face centre across the picture at a time of the file."""
    p = min(max((t_us - 5_000_000) / 1_000_000, 0), 1)
    return FACE_BEFORE + (FACE_AFTER - FACE_BEFORE) * p


def transform_at(clip, t_us):
    """The clip's x and scale at a timeline time, as the renderer reads its keyframes."""
    keys, u = clip['keyframes'], t_us - clip['startUs']
    t = clip['content']['transform']
    if not keys:
        return t['x'], t['scale']
    if u <= keys[0]['tUs']:
        k = keys[0]['transform']
        return k['x'], k['scale']
    for a, b in zip(keys, keys[1:]):
        if u <= b['tUs']:
            p = (u - a['tUs']) / max(b['tUs'] - a['tUs'], 1)
            p = p * p * (3 - 2 * p) if a['ease'] == 'smooth' else p
            lerp = lambda key: a['transform'][key] + (b['transform'][key] - a['transform'][key]) * p
            return lerp('x'), lerp('scale')
    k = keys[-1]['transform']
    return k['x'], k['scale']


def faces(project):
    """(time, left, right) of the face on the 1080 px wide canvas every quarter second."""
    out = []
    for i in range(40):
        t = 125_000 + i * 250_000
        clip = next(c for c in project['tracks'][0]['clips'] if c['startUs'] <= t < c['startUs'] + c['durationUs'])
        x, scale = transform_at(clip, t)
        # At scale 1 the wide picture fits the 1080 px width of the vertical canvas.
        width = 1080 * scale
        centre = 1080 * (0.5 + x) + (face_across(t - clip['startUs'] + clip['content']['sourceInUs']) - 0.5) * width
        out.append((t, round(centre - FACE_WIDTH / 2 * width), round(centre + FACE_WIDTH / 2 * width)))
    return out


@flow('reframe', 'Reframe a wide talking head to 9:16 from the Ratio menu: the face stays inside the frame and the '
      'safe area at every quarter second, the picture moves once and jumps at the cut, and Undo brings it back',
      before=reframe_setup)
def reframe(r):
    r.import_media(FIXTURES / 'wide-head.mp4')
    r.add_clip('wide-head.mp4')
    r.s.call("window.__nuzky.store.getState().edit({type: 'setCanvas', width: 1920, height: 1080})")
    clip = r.s.run(PROJECT)['tracks'][0]['clips'][0]
    r.s.call("window.__nuzky.store.getState().edit({type: 'splitClip', clipId: arguments[0], atUs: 7000000})", clip['id'])
    wide = wait(lambda: (p := r.s.run(PROJECT)) and len(p['tracks'][0]['clips']) == 2 and p['canvas']['width'] == 1920 and p, 10)
    r.check('a 16:9 project with the wide talking head cut at 7 s', wide, wide and wide['canvas'])
    r.s.run('window.__nuzky.store.getState().select([])')
    r.seek(2_000_000)
    time.sleep(1)
    r.shot('reframe-before')

    # Reframe beside 9:16, by keyboard: the menu opens on 16:9, and ↑ reaches the Reframe of the row above.
    r.s.run(f'document.querySelector("{RATIO}").click()')
    wait(lambda: r.s.run('return document.activeElement?.getAttribute("role") === "menuitemradio"'), 5)
    press('Up')
    focused = wait(lambda: r.s.run("""const a = document.activeElement;
        return a?.getAttribute('role') === 'menuitem' && a.textContent.trim() === 'Reframe'
            && a.closest('div.group')?.textContent.startsWith('9:16') && getComputedStyle(a.parentElement).opacity === '1'"""), 5)
    r.check('↑ from 16:9 reaches the Reframe of 9:16, which shows while focused', focused)
    r.shot('reframe-menu')
    press('Return')
    running = wait(lambda: next((j for j in r.state()['jobs'] if j['kind'] == 'reframe'), None), 10)
    r.check('Reframe starts a job and the preview says so', running and wait(
        lambda: 'Following the face' in r.s.run('return document.querySelector("section[aria-label=Preview]").innerText'), 10),
        running)
    r.shot('reframe-following')
    done = wait(lambda: (j := next((j for j in r.state()['jobs'] if j['kind'] == 'reframe'), None)) and j['status'] != 'running' and j, 120)
    r.check('the job finishes', done and done['status'] == 'done', done)

    project = r.s.run(PROJECT)
    clips = project['tracks'][0]['clips']
    r.check('the canvas is 9:16', (project['canvas']['width'], project['canvas']['height']) == (1080, 1920), project['canvas'])
    r.check('both clips fill it', all(abs(c['content']['transform']['scale'] - 1920 / 607.5) < 0.01 for c in clips),
            [c['content']['transform'] for c in clips])
    moves = [len(c['keyframes']) // 2 for c in clips]
    r.check('the picture follows the face once before the cut and holds after it', moves == [1, 0], [c['keyframes'] for c in clips])
    placed = faces(project)
    outside = [f for f in placed if f[1] < 0 or f[2] > 1080 or not SAFE[0] < (f[1] + f[2]) / 2 < SAFE[1]]
    r.check('at every quarter second the face is whole in the frame and centred inside the safe area', not outside,
            outside or placed)
    chip = wait(lambda: (t := r.s.run('return document.querySelector("section[aria-label=Preview] [role=status]")?.innerText')) and 'Reframed 2 clips to 9:16' in t and t, 10)
    r.check('the preview says what changed and offers Undo', chip and 'Undo' in chip, chip)
    for seconds in (2, 5.5, 8):
        r.seek(int(seconds * 1_000_000))
        time.sleep(1)
        r.shot(f'reframe-{seconds}s')

    if r.check('the window goes to its smallest size', resize(r, 1024, 640)):
        time.sleep(1)
        whole = r.s.run('const c = document.querySelector("section[aria-label=Preview] [role=status] span"); return c.scrollWidth <= c.clientWidth')
        r.check('the chip says it whole in the smallest window', whole)
        r.shot('reframe-1024')
        resize(r, 1440, 900)
    r.s.run("""[...document.querySelectorAll('section[aria-label=Preview] [role=status] button')]
        .find((b) => b.textContent.trim() === 'Undo').click()""")
    back = wait(lambda: (p := r.s.run(PROJECT)) and p['canvas']['width'] == 1920 and p, 10)
    r.check('Undo brings back the wide canvas and the clips as they were in one step',
            back and [c['content']['transform'] for c in back['tracks'][0]['clips']] == [c['content']['transform'] for c in wide['tracks'][0]['clips']]
            and all(not c['keyframes'] for c in back['tracks'][0]['clips']), back and back['canvas'])
    r.check('the chip is gone after Undo', wait(lambda: not r.s.run('return document.querySelector("section[aria-label=Preview] [role=status]")'), 5))
    r.check('nothing went wrong on the way', not r.errors(), r.errors())
