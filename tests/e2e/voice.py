"""Clean voice on a talking head recorded in a noisy room: turned on by keyboard in the inspector, prepared in
the background, and heard in the export as less room noise between the words and no mains hum, while the
voice keeps its level."""
import array, json, math, statistics, subprocess, time

from e2e.harness import FIXTURES, export, flow, press, wait

SOURCE = FIXTURES / 'voice.mp4'
RATE = 48_000
WINDOW = RATE // 20  # 50 ms
JOB = "return window.__nuzky.store.getState().jobs[arguments[0]] ?? null"


def decode(path, high_pass=False):
    """Mono 48 kHz float samples; with `high_pass`, above about 150 Hz, which leaves the 50 Hz hum out."""
    filters = ['-af', 'highpass=f=150,highpass=f=150'] if high_pass else []
    raw = subprocess.run(['ffmpeg', '-v', 'error', '-i', str(path), '-vn', *filters, '-ac', '1', '-ar', str(RATE),
                          '-f', 'f32le', '-'], capture_output=True, check=True).stdout
    return array.array('f', raw)


def db(power):
    return 10 * math.log10(power + 1e-12)


def window_powers(samples):
    return [sum(x * x for x in samples[i:i + WINDOW]) / WINDOW for i in range(0, len(samples) - WINDOW + 1, WINDOW)]


def level(powers, picked):
    return db(statistics.fmean(powers[i] for i in picked))


def hum_db(samples):
    """Level of 50 Hz over the whole file (Goertzel)."""
    coeff = 2 * math.cos(2 * math.pi * 50 / RATE)
    s1 = s2 = 0.0
    for x in samples:
        s1, s2 = x + coeff * s1 - s2, s1
    return db((s1 * s1 + s2 * s2 - coeff * s1 * s2) / len(samples) ** 2)


@flow('voice', 'Clean voice, turned on by keyboard in the inspector, lowers room noise and 50 Hz hum in the export and keeps the voice level')
def voice(r):
    if not SOURCE.exists():
        raise RuntimeError('tmp-test/voice.mp4 is missing: run scripts/fixtures.sh')
    r.import_media(SOURCE)
    r.add_clip('voice.mp4')
    clip = r.track()[0]
    off = export(r, 'off.mp4')

    # Keyboard: the inspector's Audio tab, then Tab from Fade out onto Clean voice and Space.
    r.focus_clip(clip['id'])
    tab = "document.querySelector('aside[aria-label=Inspector] [role=tab][aria-selected=true]')"
    if not wait(lambda: r.s.run(f'return !!{tab}'), 10):
        raise RuntimeError('the inspector shows no tabs for the clip')
    r.s.run(f'{tab}.focus()')
    r.key('End')
    r.check('End in the tab bar opens the Audio tab', wait(lambda: r.s.run(f'return {tab}?.textContent') == 'Audio', 5),
            r.s.run(f'return {tab}?.textContent'))
    r.s.run("document.querySelector('input[aria-label=\"Fade out value\"]').focus()")
    press('Tab')
    focus = "const e = document.activeElement; return e.type === 'checkbox' ? e.closest('label').textContent : e.outerHTML.slice(0, 120)"
    focused = wait(lambda: r.s.run(focus) == 'Clean voice', 3, 0.05) or r.s.run(focus)
    r.check('Tab from Fade out reaches the Clean voice checkbox', focused is True, focused)
    press('space')
    clean = "return window.__nuzky.store.getState().snap.project.tracks[0].clips[0].content.cleanVoice"
    r.check('Space turns Clean voice on', wait(lambda: r.s.run(clean), 5), r.s.run(clean))
    job = wait(lambda: (j := r.s.run(JOB, f"voice:{clip['assetId']}")) and j['status'] != 'running' and j, 60)
    r.check('the cleaned voice is prepared in the background', job and job['status'] == 'done', job)
    r.check('the progress row goes once it is ready',
            wait(lambda: not r.s.run("return [...document.querySelectorAll('aside[aria-label=Inspector] [role=status]')].some((e) => e.textContent.startsWith('Cleaning voice'))"), 5))
    r.shot('inspector-clean-voice')
    saved = lambda: json.loads(r.saved_project().read_text())['tracks'][0]['clips'][0]['content'].get('cleanVoice')
    r.check('the project file on disk has Clean voice on', wait(lambda: saved() is True, 10), saved())
    caches = list((r.work / 'cache').glob('nuzky/voice/*.f32'))
    r.check('the cleaned sound is a second cache beside the raw one', len(caches) == 1 and list((r.work / 'cache').glob('nuzky/pcm/*.f32')),
            [p.name for p in caches])
    on = export(r, 'on.mp4')

    # What a listener gets: windows between the words (quietest in the original) and the loudest windows of speech.
    plain, cleaned = window_powers(decode(off, True)), window_powers(decode(on, True))
    order = sorted(range(len(plain)), key=lambda i: plain[i])
    pauses = [i for i in order if db(plain[i]) < db(plain[order[len(order) // 10]]) + 2]
    speech = order[-len(order) // 5:]
    noise_drop = level(cleaned, pauses) - level(plain, pauses)
    voice_change = level(cleaned, speech) - level(plain, speech)
    hum_drop = hum_db(decode(on)) - hum_db(decode(off))
    r.check('room noise between the words drops by 10 dB or more', noise_drop <= -10,
            f'{noise_drop:+.1f} dB over {len(pauses)} windows ({level(plain, pauses):.1f} to {level(cleaned, pauses):.1f} dBFS)')
    r.check('50 Hz hum drops by 20 dB or more', hum_drop <= -20, f'{hum_drop:+.1f} dB')
    r.check('the voice keeps its level within 3 dB', abs(voice_change) <= 3,
            f'{voice_change:+.2f} dB over {len(speech)} windows ({level(plain, speech):.1f} to {level(cleaned, speech):.1f} dBFS)')
    # The progress row while a preparation runs. A 15 s clip is cleaned before a screenshot lands, so the job
    # event of a running preparation is replayed into the store.
    row = ("return [...document.querySelectorAll('aside[aria-label=Inspector] [role=status]')].map((e) => e.textContent)"
           ".find((t) => t.startsWith('Cleaning voice')) ?? null")
    replay = ("const st = window.__nuzky.store, id = arguments[0], job = st.getState().jobs[id];"
              "st.setState({jobs: {...st.getState().jobs, [id]: {...job, status: arguments[1], progress: arguments[2]}}});")
    r.s.run(replay, f"voice:{clip['assetId']}", 'running', 0.42)
    r.check('a running preparation shows its progress under the toggle', wait(lambda: r.s.run(row) == 'Cleaning voice · 42%', 5),
            r.s.run(row))
    r.shot('inspector-preparing')
    r.s.run(replay, f"voice:{clip['assetId']}", 'done', 1)
    r.key('z', ctrlKey=True)
    r.check('Ctrl+Z turns Clean voice off again in one step', wait(lambda: r.s.run(clean) is False, 5), r.s.run(clean))
    stops_when_turned_off(r)
    r.check('no error toast', not r.errors(), r.errors())


def stops_when_turned_off(r):
    """Turning Clean voice off while a long recording is being cleaned stops the work within seconds and leaves
    no cleaned cache or partial file behind."""
    long = r.work / 'long.m4a'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', 'anoisesrc=color=pink:amplitude=0.1:seed=5:r=48000:d=480',
                    '-ac', '2', '-c:a', 'aac', str(long)], check=True)
    r.import_media(long)
    asset = next(a['id'] for a in r.state()['assets'] if a['name'] == 'long.m4a')
    added = r.s.call("window.__nuzky.store.getState().edit({type: 'addClip', assetId: arguments[0], startUs: 0, trackId: null})", asset)
    clip_of = "return window.__nuzky.store.getState().snap.project.tracks.flatMap((t) => t.clips).find((c) => c.content.assetId === arguments[0])?.id ?? null"
    clip = wait(lambda: r.s.run(clip_of, asset), 10)
    raw = wait(lambda: (j := r.s.run(JOB, f'audio:{asset}')) and j['status'] != 'running' and j, 60)
    if not (added['ok'] and clip and raw and raw['status'] == 'done'):
        raise RuntimeError(f'the long recording was not placed and prepared: {added} {clip} {raw}')
    turn = "window.__nuzky.store.getState().edit({type: 'updateClip', clipId: arguments[0], cleanVoice: arguments[1]})"
    r.s.call(turn, clip, True)
    running = wait(lambda: (j := r.s.run(JOB, f'voice:{asset}')) and j['status'] == 'running', 10, 0.05)
    r.s.call(turn, clip, False)
    started = time.time()
    job = wait(lambda: (j := r.s.run(JOB, f'voice:{asset}')) and j['status'] != 'running' and j, 10, 0.05)
    seconds = time.time() - started
    r.check('turning Clean voice off stops the preparation of an 8-minute recording within 2 s',
            running and job and job['status'] == 'cancelled' and seconds < 2, f"{job and job['status']} after {seconds:.2f} s")
    voice = r.work / 'cache/nuzky/voice'
    left = [p.name for p in [*voice.glob(f'{asset}.*.f32'), *voice.glob('.nuzky-voice-*')]]
    r.check('a stopped preparation leaves no cleaned cache or partial file', not left, left)
