"""Lower under speech on music under a Czech talking head: turned on in the music clip's inspector, its strength
typed, saved in the project file, and heard in the export as music 18 dB down while the person speaks and back at
full level in the long pause, then taken back with Ctrl+Z."""
import array, json, math, statistics, subprocess

from e2e.harness import FIXTURES, flow, wait

SOURCE = FIXTURES / 'reel-1.mp4'
RATE = 48_000
WINDOW = RATE // 20  # 50 ms, 250 whole cycles of the tone
TONE = 5000
JOB = "return window.__nuzky.store.getState().jobs[arguments[0]] ?? null"
MUSIC = ("return window.__nuzky.store.getState().snap.project.tracks.find((t) => t.kind === 'audio')"
         "?.clips[0]?.content.duckDb ?? 0")
TYPE = """const el = document.querySelector(arguments[0]); el.focus();
Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, arguments[1]);
el.dispatchEvent(new Event('input', {bubbles: true}));
el.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true})); return true;"""
CHECKBOX = ("[...document.querySelectorAll('aside[aria-label=Inspector] label')]"
            ".find((l) => l.textContent === 'Lower under speech')?.querySelector('input')")


def export(r, name):
    target = r.work / name
    started = r.s.call('window.__nuzky.api.startExport(arguments[0], arguments[1], arguments[2], arguments[3])',
                       str(target), {'resolution': 720, 'fps': 30, 'quality': 'small'}, r.state()['epoch'], True)
    if not started['ok']:
        raise RuntimeError(f'export {name} did not start: {started}')
    job = wait(lambda: (j := r.s.run(JOB, started['value'])) and j['status'] != 'running' and j, 180)
    r.check(f'the export {name} finishes', job and job['status'] == 'done', job)
    return target


def decode(path, filters):
    raw = subprocess.run(['ffmpeg', '-v', 'error', '-i', str(path), '-vn', '-af', filters, '-ac', '1', '-ar', str(RATE),
                          '-f', 'f32le', '-'], capture_output=True, check=True).stdout
    return array.array('f', raw)


def tone_levels(samples):
    """Level of the music's tone in each 50 ms window, in dBFS (Goertzel)."""
    coeff, out = 2 * math.cos(2 * math.pi * TONE / RATE), []
    for start in range(0, len(samples) - WINDOW + 1, WINDOW):
        s1 = s2 = 0.0
        for x in samples[start:start + WINDOW]:
            s1, s2 = x + coeff * s1 - s2, s1
        out.append(10 * math.log10(4 * (s1 * s1 + s2 * s2 - coeff * s1 * s2) / WINDOW ** 2 + 1e-12))
    return out


def speech_levels(samples):
    return [10 * math.log10(sum(x * x for x in samples[i:i + WINDOW]) / WINDOW + 1e-12)
            for i in range(0, len(samples) - WINDOW + 1, WINDOW)]


@flow('ducking', 'Lower under speech, turned on in the inspector, takes music 18 dB down under speech in the export and brings it back in the pause')
def ducking(r):
    if not SOURCE.exists():
        raise RuntimeError('tmp-test/reel-1.mp4 is missing: run scripts/fixtures.sh')
    seconds = float(subprocess.run(['ffprobe', '-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', str(SOURCE)],
                                   capture_output=True, text=True, check=True).stdout)
    music = r.work / 'music.wav'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', f'sine=frequency={TONE}:sample_rate={RATE}:duration={seconds}',
                    '-c:a', 'pcm_s16le', str(music)], check=True)
    r.import_media(SOURCE, music)
    r.add_clip('reel-1.mp4')
    asset = next(a['id'] for a in r.state()['assets'] if a['name'] == 'music.wav')
    r.s.call("window.__nuzky.store.getState().edit({type: 'addClip', assetId: arguments[0], startUs: 0, trackId: null})", asset)
    clip_of = ("return window.__nuzky.store.getState().snap.project.tracks.flatMap((t) => t.clips)"
               ".find((c) => c.content.assetId === arguments[0])?.id ?? null")
    clip = wait(lambda: r.s.run(clip_of, asset), 10)
    if not clip:
        raise RuntimeError('the music was not placed')
    off = export(r, 'off.mp4')

    r.focus_clip(clip)
    checked = lambda: r.s.run(f'return {CHECKBOX}?.checked ?? null')
    r.check('the music clip offers Lower under speech, off', wait(lambda: checked() is not None, 10) and checked() is False, checked())
    r.check('the strength stays hidden while it is off', not r.s.run("return !!document.querySelector('input[aria-label=\"Lower by value\"]')"))
    r.s.run(f'{CHECKBOX}.click()')
    r.check('checking it lowers the music by 12 dB', wait(lambda: r.s.run(MUSIC) == 12, 5), r.s.run(MUSIC))
    r.s.run(TYPE, 'input[aria-label="Lower by value"]', '18')
    r.check('typing 18 in Lower by sets 18 dB', wait(lambda: r.s.run(MUSIC) == 18, 5), r.s.run(MUSIC))
    r.shot('inspector-lower-under-speech')
    saved = lambda: next(t for t in json.loads(r.saved_project().read_text())['tracks'] if t['kind'] == 'audio')['clips'][0]['content'].get('duckDb')
    r.check('the project file on disk has the music at 18 dB under speech', wait(lambda: saved() == 18, 10), saved())
    on = export(r, 'on.mp4')

    # What a listener gets: the music's level with ducking against without it, window by window, where the person
    # speaks and in the middle of the 2.4 s pause. Speech and pauses come from the talking head alone, which plays
    # from 0 s.
    gain = [b - a for a, b in zip(tone_levels(decode(off, f'highpass=f={TONE - 500},highpass=f={TONE - 500}')),
                                  tone_levels(decode(on, f'highpass=f={TONE - 500},highpass=f={TONE - 500}')))]
    voice = speech_levels(decode(SOURCE, 'anull'))
    loudest = max(voice)
    speech = [i for i, v in enumerate(voice) if v > loudest - 15]
    quiet = [v < loudest - 35 for v in voice]
    runs, start = [], None
    for i, q in enumerate(quiet + [False]):
        if q and start is None:
            start = i
        elif not q and start is not None:
            runs.append((start, i))
            start = None
    first, last = max(runs, key=lambda run: run[1] - run[0])
    # Back at full level 650 ms after the last word (250 ms hold, 400 ms release); going down 150 ms before the next.
    pause = list(range(first + 14, last - 4))
    under = statistics.median(gain[i] for i in speech)
    back = statistics.median(gain[i] for i in pause)
    held = sum(abs(gain[i] + 18) < 2 for i in speech) / len(speech)
    r.check('the music is 18 dB down while the person speaks', abs(under + 18) < 1.5,
            f'{under:+.1f} dB over {len(speech)} windows of speech, {held:.0%} of them within 2 dB')
    r.check('it stays down through the gaps between words', held >= 0.9, f'{held:.0%} of speech windows within 2 dB of -18')
    r.check('the music is back at full level in the long pause', abs(back) < 1,
            f'{back:+.1f} dB over {len(pause)} windows, {first * WINDOW / RATE:.2f}-{last * WINDOW / RATE:.2f} s quiet')

    r.focus_clip(clip)
    r.key('z', ctrlKey=True)
    r.check('Ctrl+Z takes the strength back to 12 dB', wait(lambda: r.s.run(MUSIC) == 12, 5), r.s.run(MUSIC))
    r.key('z', ctrlKey=True)
    r.check('a second Ctrl+Z turns it off', wait(lambda: r.s.run(MUSIC) == 0, 5), r.s.run(MUSIC))
    r.check('no error toast', not r.errors(), r.errors())
