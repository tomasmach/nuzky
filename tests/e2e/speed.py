"""Keep pitch on a Czech talking head sped up to 1.25x in the inspector: the voice in the export is as high as in the
recording, 1.25 times shorter, and every phrase starts where speed puts it, which is where captions and cuts land.
Turned off by keyboard, the voice rises with the speed again, and it stays off when the speed changes."""
import array, json, math, statistics, subprocess

from e2e.harness import FIXTURES, export, flow, press, wait

SOURCE = FIXTURES / 'reel-1.mp4'
RATE = 16_000
CONTENT = "return window.__nuzky.store.getState().snap.project.tracks[0].clips[0].content"
CHECKBOX = ("[...document.querySelectorAll('aside[aria-label=Inspector] label')]"
            ".find((l) => l.textContent === 'Keep pitch')?.querySelector('input')")
TYPE = """const el = document.querySelector(arguments[0]); el.focus();
Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, arguments[1]);
el.dispatchEvent(new Event('input', {bubbles: true}));
el.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true})); return true;"""


def decode(path):
    raw = subprocess.run(['ffmpeg', '-v', 'error', '-i', str(path), '-vn', '-ac', '1', '-ar', str(RATE), '-f', 'f32le', '-'],
                         capture_output=True, check=True).stdout
    return array.array('f', raw)


def levels(samples, window=RATE // 100):
    """dBFS of each 10 ms."""
    return [10 * math.log10(sum(x * x for x in samples[i:i + window]) / window + 1e-12)
            for i in range(0, len(samples) - window + 1, window)]


def pitch(samples):
    """Median pitch in Hz of the loudest 40 ms frames of speech, by autocorrelation between 70 and 400 Hz."""
    frame, found = RATE // 25, []
    loud = levels(samples, frame)
    for index in sorted(range(len(loud)), key=lambda i: loud[i])[-40:]:
        x = samples[index * frame:(index + 1) * frame]
        mean = sum(x) / frame
        x = [v - mean for v in x]
        corr = {lag: sum(a * b for a, b in zip(x, x[lag:])) for lag in range(RATE // 400 - 1, RATE // 70 + 2)}
        lag = max(range(RATE // 400, RATE // 70 + 1), key=corr.get)
        energy = sum(v * v for v in x)
        if corr[lag] < 0.4 * energy:
            continue
        # The peak between samples, through a parabola.
        a, b, c = corr[lag - 1], corr[lag], corr[lag + 1]
        found.append(RATE / (lag + 0.5 * (a - c) / (a - 2 * b + c)))
    return statistics.median(found), len(found)


def onsets(samples):
    """Seconds where speech starts after at least 200 ms of quiet."""
    loud = levels(samples)
    top = max(loud)
    found, quiet = [], 20
    for i, level in enumerate(loud):
        if level > top - 30:
            if quiet >= 20:
                found.append(i / 100)
            quiet = 0
        else:
            quiet += 1
    return found


@flow('speed', 'Keep pitch keeps a voice sped up to 1.25x as high as recorded in the export, with phrases where speed puts them; off, it rises again')
def speed(r):
    if not SOURCE.exists():
        raise RuntimeError('tmp-test/reel-1.mp4 is missing: run scripts/fixtures.sh')
    r.import_media(SOURCE)
    r.add_clip('reel-1.mp4')
    clip = r.track()[0]
    r.focus_clip(clip['id'])
    tab = "[...document.querySelectorAll('aside[aria-label=Inspector] [role=tab]')].find((t) => t.textContent === 'Speed')"
    if not wait(lambda: r.s.run(f'return !!{tab}'), 10):
        raise RuntimeError('the inspector has no Speed tab for the clip')
    r.s.run(f'{tab}.click()')
    checked = lambda: r.s.run(f'return {CHECKBOX}?.checked ?? null')
    r.check('Keep pitch is not offered at 1x', wait(lambda: r.s.run('return !!document.querySelector(\'input[aria-label="Speed value"]\')'), 5)
            and checked() is None, checked())
    r.s.run(TYPE, 'input[aria-label="Speed value"]', '1.25')
    content = lambda: r.s.run(CONTENT)
    r.check('typing 1.25 speeds the clip up with Keep pitch on',
            wait(lambda: content()['speed'] == 1.25 and content().get('keepPitch') is True, 5), content())
    r.check('the inspector shows Keep pitch checked', wait(lambda: checked() is True, 5), checked())
    r.shot('inspector-keep-pitch')
    saved = lambda: next((c['content'].get('keepPitch') for t in json.loads(r.saved_project().read_text())['tracks']
                          for c in t['clips']), None)
    r.check('the project file on disk has Keep pitch on', wait(lambda: saved() is True, 10), saved())
    kept = export(r, 'kept.mp4')

    # What a listener gets: the voice's pitch, the length, and where each phrase starts.
    source, sped = decode(SOURCE), decode(kept)
    (recorded, frames), (heard, heard_frames) = pitch(source), pitch(sped)
    r.check('the voice in the export is as high as recorded, within 3 %', abs(heard / recorded - 1) < 0.03,
            f'{recorded:.1f} Hz recorded, {heard:.1f} Hz exported ({frames} and {heard_frames} voiced frames)')
    length, expected = len(sped) / RATE, len(source) / RATE / 1.25
    r.check('the export is 1.25 times shorter', abs(length - expected) < 0.06, f'{length:.3f} s, expected {expected:.3f} s')
    starts, placed = [t / 1.25 for t in onsets(source)], onsets(sped)
    worst = max(min(abs(t - p) for p in placed) for t in starts)
    r.check('every phrase starts where speed puts it, within 25 ms', len(starts) == len(placed) and worst <= 0.025,
            f'{len(starts)} phrases recorded, {len(placed)} exported, furthest {worst * 1000:.0f} ms off')

    r.s.run(f'{CHECKBOX}.focus()')
    press('space')
    r.check('Space turns Keep pitch off', wait(lambda: content().get('keepPitch', False) is False, 5), content())
    preset = ("[...document.querySelectorAll('aside[aria-label=Inspector] button')]"
              ".find((b) => b.textContent === '1.5x')")
    r.s.run(f'{preset}.click()')
    r.check('another speed leaves Keep pitch off', wait(lambda: content()['speed'] == 1.5, 5)
            and content().get('keepPitch', False) is False and checked() is False, content())
    raised = export(r, 'raised.mp4')
    higher, _ = pitch(decode(raised))
    r.check('without Keep pitch the voice rises with the speed', abs(higher / recorded - 1.5) < 0.05,
            f'{recorded:.1f} Hz recorded, {higher:.1f} Hz at 1.5x')

    r.focus_clip(clip['id'])
    r.key('z', ctrlKey=True)
    r.key('z', ctrlKey=True)
    r.check('two Ctrl+Z bring back 1.25x with Keep pitch on',
            wait(lambda: content()['speed'] == 1.25 and content().get('keepPitch') is True, 5), content())
    r.check('no error toast', not r.errors(), r.errors())
