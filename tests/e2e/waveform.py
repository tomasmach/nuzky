"""A one-hour recording on the timeline: the waveform of the part on screen shows while the sound of the file is
still being prepared, and every bar on screen matches the sound FFmpeg decodes from the whole file, at the start
and half an hour in. Clips of the same file draw from the same loaded blocks. The time to the first waveform and
the app's memory go to tmp-test/repro/waveform/measure.json."""
import json, subprocess, threading, time
from pathlib import Path

import numpy as np

from e2e.harness import APP, FIXTURES, MACOS, flow, wait, webdriver

SOURCE = FIXTURES / 'hour.m4a'
RATE, PER_SECOND = 48_000, 50
BUCKET = RATE // PER_SECOND
HEADER_W = 140
# The first screen (about 20 s of sound) decodes in a few milliseconds, and the timeline asks again every 250 ms
# while what it shows is still being decoded. 1 s leaves room for a slow machine; the whole hour takes 1.5 to 3 s.
FIRST_WAVEFORM_S = 1.0

# For each waveform canvas of the clip: its pixel columns on screen, what each one shows (the bar height) and
# which stretch of the clip it stands for.
BARS = """const clip = document.querySelector(`[data-clip-id='${arguments[0]}']`);
if (!clip) return null;
const scroller = clip.closest('.overflow-auto');
const lane = scroller.getBoundingClientRect();
const box = clip.getBoundingClientRect();
const from = Math.max(lane.left + arguments[1], box.left), to = Math.min(lane.right, box.right);
const columns = [];
for (const canvas of clip.querySelectorAll('canvas')) {
  const rect = canvas.getBoundingClientRect();
  if (!canvas.width || !rect.width) continue;
  const scale = rect.width / canvas.width;
  const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data;
  for (let x = 0; x < canvas.width; x++) {
    const centre = rect.left + (x + 0.5) * scale;
    if (centre < from || centre >= to) continue;
    let bar = 0;
    for (let y = 0; y < canvas.height; y++) if (pixels[(y * canvas.width + x) * 4 + 3] > 0) bar++;
    columns.push({local: rect.left - box.left + x * scale, scale, bar, height: canvas.height});
  }
}
return {columns, width: box.width, lane: to - from};"""

CLIP = """const st = window.__nuzky.store.getState();
for (const t of st.snap.project.tracks) for (const c of t.clips)
  if (c.content.type === 'media' && c.content.assetId === arguments[0])
    return {id: c.id, startUs: c.startUs, durationUs: c.durationUs, sourceInUs: c.content.sourceInUs, speed: c.content.speed};
return null;"""


def reference(start_s, seconds):
    """Peaks of `seconds` of sound from `start_s`, decoded by FFmpeg from the start of the file: nothing is seeked,
    so the samples are those of a whole-file decode."""
    first, count = start_s * PER_SECOND, seconds * PER_SECOND
    process = subprocess.Popen(['ffmpeg', '-v', 'error', '-i', str(SOURCE), '-t', str(start_s + seconds), '-f', 'f32le',
                                '-ac', '2', '-ar', str(RATE), '-'], stdout=subprocess.PIPE)
    skip = first * BUCKET * 2 * 4
    while skip > 0:
        skip -= len(process.stdout.read(min(skip, 1 << 24)))
    samples = np.frombuffer(process.stdout.read(count * BUCKET * 2 * 4), dtype='<f4')
    process.wait()
    usable = len(samples) // (BUCKET * 2) * BUCKET * 2
    peaks = np.abs(samples[:usable]).reshape(-1, BUCKET * 2).max(axis=1)
    return first, (np.minimum(peaks, 1.0) * 255).astype(np.uint8)


def compare(seen, clip, peaks_from, peaks):
    """Bars that differ from the ones the reference peaks draw, the way Waveform.tsx draws them."""
    first = clip['sourceInUs'] / 1e6 * PER_SECOND
    span = clip['durationUs'] * clip['speed'] / 1e6 * PER_SECOND
    width = seen['width']
    wrong, worst = 0, 0
    for column in seen['columns']:
        a = int(np.floor(first + column['local'] / width * span))
        b = max(a + 1, int(np.floor(first + (column['local'] + column['scale']) / width * span)))
        window = peaks[a - peaks_from:b - peaks_from]
        if a < peaks_from or len(window) < b - a:
            raise RuntimeError(f'the reference does not cover peaks {a}..{b}')
        expected = max(1, round(np.sqrt(int(window.max()) / 255) * column['height']))
        worst = max(worst, abs(column['bar'] - expected))
        wrong += column['bar'] != expected
    return wrong, worst


class Memory(threading.Thread):
    """Samples the resident memory of the app and its web process every 50 ms."""

    def __init__(self, r):
        super().__init__(daemon=True)
        self.r, self.data_home, self.samples, self.stopped = r, str(r.work / 'data'), [], False

    def processes(self):
        if MACOS:
            return {self.r.app.pid: 'app', webdriver('GET', self.r.s.path + '/nuzky/web-process'): 'WebContent'}
        found = {}
        for proc in Path('/proc').iterdir():
            try:
                if not proc.name.isdigit() or f'XDG_DATA_HOME={self.data_home}'.encode() not in (proc / 'environ').read_bytes():
                    continue
                name = 'app' if Path(proc / 'exe').resolve() == APP.resolve() else (proc / 'comm').read_text().strip()
                found[int(proc.name)] = name
            except OSError:
                continue
        return found

    @staticmethod
    def rss_mb(pid):
        if MACOS:
            kb = subprocess.run(['ps', '-o', 'rss=', '-p', str(pid)], capture_output=True, text=True).stdout.strip()
            return int(kb) / 1024 if kb else None
        try:
            line = next(l for l in Path(f'/proc/{pid}/status').read_text().splitlines() if l.startswith('VmRSS:'))
            return int(line.split()[1]) / 1024
        except (OSError, StopIteration):
            return None

    def run(self):
        processes = self.processes()
        while not self.stopped:
            sample = {'t': time.time()}
            for pid, name in processes.items():
                if (mb := self.rss_mb(pid)) is not None:
                    sample[name] = max(sample.get(name, 0), mb)
            self.samples.append(sample)
            time.sleep(0.05)

    def peak(self, name, since=0.0, until=float('inf')):
        values = [s[name] for s in self.samples if since <= s['t'] <= until and name in s]
        return round(max(values), 1) if values else None

    def at(self, name, moment):
        values = [s[name] for s in self.samples if s['t'] <= moment and name in s]
        return round(values[-1], 1) if values else None


LANE = """const clip = document.querySelector(`[data-clip-id='${arguments[0]}']`);
return Math.floor(clip.closest('.overflow-auto').clientWidth - 140);"""
LOADED = "return Object.keys(window.__nuzky.store.getState().waveforms[arguments[0]] ?? {}).map(Number)"
# Blocks the store keeps beyond those on screen (src/lib/store.ts), and the blocks of the hour.
KEPT_BLOCKS, HOUR_BLOCKS = 256, 3600 * PER_SECOND // 512 + 1


def scroll(r, clip_id, left):
    r.s.run("document.querySelector(`[data-clip-id='${arguments[0]}']`).closest('.overflow-auto').scrollLeft = arguments[1]",
            clip_id, left)


def visible_bars(r, clip_id):
    seen = r.s.run(BARS, clip_id, HEADER_W)
    # Every pixel column of the clip on screen has its bar: blocks still missing leave columns empty.
    if not seen or not seen['columns']:
        return None
    covered = sum(c['scale'] for c in seen['columns'])
    return seen if covered >= seen['lane'] - max(c['scale'] for c in seen['columns']) - 1 and all(
        c['bar'] > 0 for c in seen['columns']) else None


def check_against_reference(r, name, seen, clip, start_s):
    seconds = int(seen['lane'] / r.s.run('return window.__nuzky.store.getState().zoom')) + 4
    peaks_from, peaks = reference(start_s, seconds)
    wrong, worst = compare(seen, clip, peaks_from, peaks)
    r.check(f'{name}: all {len(seen["columns"])} bars on screen match the whole-file decode',
            wrong == 0, {'wrong': wrong, 'largest difference px': worst})


@flow('waveform', 'One-hour recording: the waveform on screen shows while the sound is prepared and matches FFmpeg')
def waveform(r):
    memory = Memory(r)
    memory.start()
    time.sleep(1)
    started = time.time()
    r.s.call('window.__nuzky.importPaths(arguments[0], {trackId: null, startUs: 0})', [str(SOURCE)])
    asset = wait(lambda: next((a['id'] for a in r.state()['assets'] if a['name'] == SOURCE.name), None), 30, 0.05)
    clip = wait(lambda: r.s.run(CLIP, asset), 30, 0.05)
    if not clip:
        raise RuntimeError('the recording was not placed on the timeline')
    r.s.run("window.__nuzky.store.getState().select([])")
    job = f'audio:{asset}'
    status = lambda: r.s.run('return window.__nuzky.store.getState().jobs[arguments[0]]?.status ?? null', job)

    seen = wait(lambda: visible_bars(r, clip['id']), 600, 0.05)
    first_waveform = time.time() - started
    decoding = status() == 'running'
    r.shot('1-first-waveform')
    r.check(f'the waveform on screen shows within {FIRST_WAVEFORM_S} s of the import', seen and first_waveform <= FIRST_WAVEFORM_S,
            f'{first_waveform:.2f} s')
    r.check('it shows while the sound of the file is still being prepared', decoding)
    finished = wait(lambda: status() in ('done', 'failed', 'cancelled'), 600, 0.05)
    audio_ready = time.time() - started
    r.check('the sound of the hour is prepared', finished and status() == 'done', status())
    if seen:
        check_against_reference(r, 'first screen, shown during the preparation', seen, clip, 0)

    # Half an hour in, once the file is prepared: the blocks there load when they come into view.
    zoom = r.s.run('return window.__nuzky.store.getState().zoom')
    scrolled = time.time()
    scroll(r, clip['id'], 1800 * zoom)
    middle = wait(lambda: visible_bars(r, clip['id']), 30, 0.05)
    middle_s = time.time() - scrolled
    middle_done = time.time()
    r.shot('2-half-an-hour-in')
    r.check('half an hour in, the waveform on screen shows within 1 s of scrolling there', middle and middle_s <= 1.0, f'{middle_s:.2f} s')
    if middle:
        check_against_reference(r, 'half an hour in', middle, clip, 1800)

    # Zoomed out, the whole hour passes by: blocks scrolled away go once more than the store keeps are loaded.
    r.s.run("window.__nuzky.store.setState({panelTab: 'media'}); window.__nuzky.store.getState().setZoom(4)")
    wait(lambda: r.s.run('return window.__nuzky.store.getState().zoom') == 4, 5)
    lane = r.s.run(LANE, clip['id'])
    for left in range(0, 3600 * 4 + lane, lane):
        scroll(r, clip['id'], left)
        if not wait(lambda: visible_bars(r, clip['id']), 10, 0.05):
            r.check(f'zoomed out, the waveform at {left / 4 / 60:.0f} min draws', False)
            break
    loaded = r.s.run(LOADED, asset)
    # A clip draws its window of the screen plus up to 1000 px around it, and loads a block more on each side.
    on_screen = int(np.ceil((lane + 1000) / 4 * PER_SECOND / 512)) + 2
    r.check(f'after the whole hour zoomed out, at most {KEPT_BLOCKS} blocks besides those on screen stay loaded, '
            'and the start is no longer among them', len(loaded) <= KEPT_BLOCKS + on_screen and 0 not in loaded,
            {'loaded': len(loaded), 'of the hour': HOUR_BLOCKS, 'on screen at most': on_screen})
    scroll(r, clip['id'], 0)
    again = wait(lambda: visible_bars(r, clip['id']), 10, 0.05)
    r.shot('3-back-at-the-start-zoomed-out')
    r.check('back at the start, its waveform draws again', again)
    if again:
        check_against_reference(r, 'the start again, zoomed out', again, clip, 0)

    # The Audio tab's overview of the whole hour loads all its blocks and fits its row; closing it lets the store
    # drop the blocks past the cap without waiting for another block to arrive.
    r.s.run("window.__nuzky.store.setState({panelTab: 'audio'})")
    overview = wait(lambda: len(r.s.run(LOADED, asset)) >= HOUR_BLOCKS, 10)
    fits = r.s.run("""const canvas = [...document.querySelectorAll('canvas')].find((c) => !c.closest('[data-clip-id]') && c.width > 0);
        if (!canvas) return null;
        const own = canvas.getBoundingClientRect(), row = canvas.parentElement.getBoundingClientRect();
        return own.left >= row.left - 0.5 && own.right <= row.right + 0.5;""")
    r.shot('4-audio-tab')
    r.check('the Audio tab shows the whole hour inside its row', overview and fits, {'loaded': len(r.s.run(LOADED, asset)), 'fits': fits})
    r.s.run("window.__nuzky.store.setState({panelTab: 'media'})")
    dropped = wait(lambda: len(r.s.run(LOADED, asset)) <= KEPT_BLOCKS + on_screen, 5)
    r.check('closing the Audio tab drops the blocks past the cap', dropped, {'loaded': len(r.s.run(LOADED, asset))})

    # Splitting gives two clips of one file; the second draws from the blocks already loaded.
    r.s.run("""const api = window.__nuzky.api; window.__waveRequests = 0;
        if (!api.__counted) { const load = api.waveform; api.waveform = (...a) => { window.__waveRequests++; return load(...a); }; api.__counted = true; }""")
    t = int(r.s.run(LANE, clip['id']) / 2 / 4 * 1_000_000)
    r.s.call('window.__nuzky.store.getState().edit({type: "splitClip", clipId: arguments[0], atUs: arguments[1]})', clip['id'], t)
    halves = wait(lambda: (h := [c for tr in r.state()['tracks'] for c in tr['clips'] if c['assetId'] == asset]) and len(h) == 2 and h, 10)
    both = halves and wait(lambda: all(visible_bars(r, c['id']) for c in halves), 10)
    r.shot('5-split')
    requests = r.s.run('return window.__waveRequests')
    r.check('both halves of the split draw their waveform from the blocks already loaded', both and requests == 0, {'requests': requests})

    webdriver('POST', r.s.path + '/window/rect', {'width': 1024, 'height': 640})
    if r.check('the window gets to 1024 x 640', wait(lambda: r.s.run('return window.innerWidth') == 1024, 5)):
        small = halves and all(wait(lambda: visible_bars(r, c['id']), 10, 0.05) for c in halves)
        r.shot('6-small-window')
        r.check('in the smallest window the waveform on screen draws whole', small)

    memory.stopped = True
    memory.join()
    web = {k for s in memory.samples for k in s} - {'t', 'app'}
    measure = {
        'first_waveform_s': round(first_waveform, 2),
        'audio_ready_s': round(audio_ready, 2),
        'middle_after_scroll_s': round(middle_s, 2),
        'app_rss_before_import_mb': memory.at('app', started),
        'app_rss_peak_until_first_waveform_mb': memory.peak('app', started, started + first_waveform),
        'app_rss_peak_mb': memory.peak('app', started),
        'web_rss_peak_until_half_an_hour_in_mb': max((memory.peak(n, started, middle_done) or 0) for n in web) or None,
        'web_rss_peak_mb': max((memory.peak(n, started) or 0) for n in web) or None,
    }
    (r.work / 'measure.json').write_text(json.dumps(measure, indent=2) + '\n')
    print(f'  measure: {measure}', flush=True)
    growth = (measure['app_rss_peak_mb'] or 0) - (measure['app_rss_before_import_mb'] or 0)
    # Preparing the sound streams it to disk; drawing reads blocks of peaks. Neither holds the hour in memory
    # (1.4 GB as 48 kHz stereo float).
    r.check('the app never holds the hour of sound in memory: it grows by less than 200 MB', growth < 200, f'{growth:.0f} MB')
