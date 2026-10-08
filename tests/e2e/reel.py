"""The whole way from raw takes to a reel: an agent connected over MCP cuts three Czech takes in the open app
and exports them with the Reels preset. What it checks is what the viewer gets: the last attempt of every
sentence, no fillers or long pauses, every kept phrase whole, captions inside the safe area, and a file that
Instagram and TikTok take as it is."""
import array, difflib, json, math, re, subprocess, time, unicodedata

from e2e.harness import (ANALYZE, FIXTURES, MODELS, Bridge, flow, link_models, preview_crop, preview_rect, wait)

TAKES = [FIXTURES / f'reel-{n}.mp4' for n in (1, 2, 3)]
MODEL = 'large-v3-turbo-q5_0'
RATE = 48_000
SPEECH_DB = -42  # Room tone sits near -64 dBFS, espeak speech well above -30.
FILLER_GAP_S = 0.18  # espeak pauses about 0.2 s at a comma; inside the rest of a phrase at most 0.14 s.
SAFE = (60, 250, 900, 1420)  # The Reels and TikTok safe area of a 1080x1920 frame: left, top, right, bottom.


def phrases():
    """The phrases of tests/e2e/reel_takes.tsv in take order: (take, pause_s, role, text)."""
    rows = (line.split('\t') for line in (FIXTURES.parent / 'tests/e2e/reel_takes.tsv').read_text().splitlines()
            if line and not line.startswith('#'))
    return [(int(take), float(pause), role, text) for take, pause, role, text in rows]


def pcm(path, start_s=0.0, seconds=None):
    """Mono 48 kHz samples, -1..1, as FFmpeg decodes the file."""
    command = ['ffmpeg', '-v', 'error', '-ss', str(start_s), '-i', str(path)]
    command += ['-t', str(seconds)] if seconds else []
    raw = subprocess.run(command + ['-vn', '-ac', '1', '-ar', str(RATE), '-f', 's16le', '-'], capture_output=True,
                         check=True).stdout
    return [x / 32768 for x in array.array('h', raw)]


def db(samples):
    return 20 * math.log10(math.sqrt(sum(x * x for x in samples) / max(1, len(samples))) + 1e-9)


def spans(samples, gap_s):
    """Where speech is, in 10 ms frames, joining pieces closer than `gap_s`: [(start_s, end_s)]."""
    hop = RATE // 100
    loud = [db(samples[i:i + hop]) > SPEECH_DB for i in range(0, len(samples) - hop + 1, hop)]
    found = []
    for i, on in enumerate(loud):
        if on and found and i / 100 - found[-1][1] <= gap_s:
            found[-1][1] = (i + 1) / 100
        elif on:
            found.append([i / 100, (i + 1) / 100])
    return [tuple(s) for s in found]


def truth():
    """Every phrase where it really is in its take, measured in the sound: (take, role, text, start_s, end_s).
    A sentence that starts with a filler becomes the filler and the rest, split at its first pause."""
    found = []
    for n, path in enumerate(TAKES, 1):
        samples = pcm(path)
        pieces = spans(samples, 0.5)  # Every pause of the takes is longer than half a second.
        mine = [p for p in phrases() if p[0] == n]
        assert len(pieces) == len(mine), f'take {n}: {len(pieces)} spoken phrases for {len(mine)} in the list'
        for (_, _, role, text), (start, end) in zip(mine, pieces):
            if role != 'filler-lead':
                found.append((n, role, text, start, end))
                continue
            inner = spans(samples[int(start * RATE):int(end * RATE)], 0.01)
            split = next(i for i in range(1, len(inner)) if inner[i][0] - inner[i - 1][1] >= FILLER_GAP_S)
            filler, rest = text.split(', ', 1)
            found.append((n, 'filler', filler, start, start + inner[split - 1][1]))
            found.append((n, 'keep', rest, start + inner[split][0], end))
    return found


def played(state, assets):
    """For each take, the source time ranges the main track plays, and the timeline order of what it plays."""
    ranges = {n: [] for n in assets.values()}
    order = []
    main = next(t for t in state['tracks'] if t['id'] == 'main')
    for clip in sorted(main['clips'], key=lambda c: c['startUs']):
        content = clip['content']
        take = assets[content['assetId']]
        start = content['sourceInUs'] / 1e6
        end = start + clip['durationUs'] * content.get('speed', 1) / 1e6
        ranges[take].append((start, end))
        order.append((take, start, end, clip['startUs'] / 1e6))
    return ranges, order


def heard(ranges, start, end):
    return sum(max(0.0, min(end, b) - max(start, a)) for a, b in ranges)


def words(text):
    """Lower case, no punctuation or diacritics, digits as they are: what recognition can be compared on."""
    text = unicodedata.normalize('NFKD', text.lower())
    return re.findall(r'[a-z0-9]+', ''.join(c for c in text if not unicodedata.combining(c)))


def recognise(path, work):
    out = subprocess.run([str(ANALYZE), str(path), 'words', '--lang', 'cs', '--model', str(MODELS / f'ggml-{MODEL}.bin'),
                          '--cache', str(work / 'analysis-cache')], capture_output=True, encoding='utf-8', errors='replace', check=True,
                         timeout=600).stdout
    return json.loads(out)['words']


def poll(bridge, job, timeout):
    end = time.time() + timeout
    while True:
        state = bridge.call('job', {'job_id': job['job_id'], 'action': 'get'})
        if state['status'] != 'running' or time.time() > end:
            return state
        time.sleep(1)


def probe(path):
    return json.loads(subprocess.run(['ffprobe', '-v', 'error', '-show_streams', '-show_format', '-of', 'json', str(path)],
                                     capture_output=True, text=True, check=True).stdout)


def atoms(path):
    """Top-level MP4 boxes in file order."""
    found = []
    with open(path, 'rb') as f:
        while header := f.read(8):
            size, kind = int.from_bytes(header[:4], 'big'), header[4:].decode('latin-1')
            if size == 1:
                size = int.from_bytes(f.read(8), 'big') - 8
            found.append(kind)
            if size < 8:
                break
            f.seek(size - 8, 1)
    return found


def loudness(path):
    """Integrated loudness (LUFS) and true peak (dBTP) as FFmpeg's EBU R128 meter reports them."""
    log = subprocess.run(['ffmpeg', '-nostdin', '-hide_banner', '-i', str(path), '-vn', '-af', 'ebur128=peak=true',
                          '-f', 'null', '-'], capture_output=True, text=True, timeout=300).stderr
    summary = log[log.rindex('Summary:'):]
    return (float(re.search(r'I:\s+(-?[\d.]+) LUFS', summary)[1]), float(re.search(r'Peak:\s+(-?[\d.]+) dBFS', summary)[1]))


def caption_box(frame):
    """Bounding box of caption white in an exported frame: the takes are dark, captions white with a black outline."""
    from PIL import Image
    image = Image.open(frame).convert('L')
    white = image.point(lambda v: 255 if v > 200 else 0)
    return white.getbbox(), sum(white.histogram()[255:])


def reel_setup(r):
    link_models(r, ('ggml-silero-v5.1.2.bin', f'ggml-{MODEL}.bin'))


@flow('reel', 'An agent turns three raw Czech takes into a Reels MP4 over MCP: last attempts only, no fillers or long '
      'pauses, whole words, Reel captions in the safe area, Reels loudness and format', before=reel_setup)
def reel(r):
    truths = truth()
    r.check('the app starts with an empty vertical project', not r.track())
    bridge = Bridge(r, r.saved_project())
    try:
        rough_cut(r, bridge, truths)
    finally:
        bridge.close()


def rough_cut(r, bridge, truths):
    run = bridge.call('begin_run', {'label': 'Rough cut'})['run_id']
    r.check('the UI shows the agent editing', wait(lambda: r.state()['aiRun'], 10))
    ids = bridge.call('import_media', {'run_id': run, 'paths': [str(p) for p in TAKES], 'request_id': 'takes'})['asset_ids']
    assets = {asset: n for n, asset in enumerate(ids, 1)}
    bridge.call('apply_edits', {'run_id': run, 'request_id': 'place', 'edits': [
        {'type': 'addClip', 'assetId': asset, 'trackId': 'main', 'startUs': None} for asset in ids]})
    r.check('the three takes appear on the main track live', wait(lambda: len(r.track()) == 3, 10), r.track())

    started = time.time()
    job = poll(bridge, bridge.call('transcribe', {'language': 'cs', 'model': MODEL}), 600)
    r.check('Czech speech in all three takes is recognised', job['status'] == 'done', job)
    print(f'  recognition took {time.time() - started:.0f} s', flush=True)
    transcript = bridge.call('get_transcript', {})
    r.check('no take is left untranscribed', not transcript['untranscribed'], transcript['untranscribed'])

    analysis = bridge.call('analyze', {'kind': 'retakes'})
    again = bridge.call('analyze', {'kind': 'retakes'})
    r.check('the retake analysis is deterministic', analysis == again)
    text = [w['text'] for w in transcript['words']]
    groups = [[' '.join(text[s['from']:s['to'] + 1]) for s in g['sentences']] for g in analysis['groups']]
    kept = [' '.join(text[g['sentences'][g['keep']]['from']:g['sentences'][g['keep']]['to'] + 1]) for g in analysis['groups']]
    r.check('it finds both restarted sentences', len(groups) == 2 and [len(g) for g in groups] == [3, 2], groups)
    r.check('and recommends the last complete attempt of each',
            [words(k)[:3] for k in kept] == [['dneska', 'vam', 'ukazu'], ['kamera', 'musi', 'stat']]
            and all(g['keep'] == len(g['sentences']) - 1 for g in analysis['groups']), kept)
    fillers = [' '.join(text[f['from']:f['to'] + 1]) for f in analysis['fillers']]
    r.check('it finds the fillers that start a sentence', [words(f) for f in fillers] == [['ehm'], ['jakoby'], ['proste']],
            fillers)

    delete = analysis['suggested_delete']
    plan = bridge.call('edit_transcript', {'transcript_key': transcript['transcript_key'], 'delete': delete, 'dry_run': True})
    cut = bridge.call('edit_transcript', {'run_id': run, 'request_id': 'rough-cut', 'transcript_key': transcript['transcript_key'],
                                          'delete': delete})
    r.check('the cut matches its dry run', cut['duration_us'] == plan['duration_us'], [plan['duration_us'], cut['duration_us']])
    captions = bridge.call('build_captions', {'run_id': run})
    r.check('Reel captions are built', captions['caption_count'] > 10, captions)
    r.check('the cut and the captions show live while the agent works',
            wait(lambda: len(r.track()) > 3 and len(r.state()['tracks']) == 2, 10), r.track())
    r.shot('agent-editing')
    bridge.call('end_run', {'run_id': run, 'action': 'keep'})
    r.check('the lock ends with the run', wait(lambda: not r.state()['aiRun'], 10))

    state = bridge.call('get_state', {})
    ranges, order = played(state, assets)
    whole = [(t, text, round(heard(ranges[t], s, e) - (e - s), 3)) for t, role, text, s, e in truths if role == 'keep']
    r.check('every kept phrase plays whole', all(missing > -0.002 for *_, missing in whole),
            [w for w in whole if w[2] <= -0.002])
    gone = [(t, role, text, round(heard(ranges[t], s, e), 3)) for t, role, text, s, e in truths if role != 'keep']
    r.check('earlier attempts and fillers are cut', all(h <= 0.01 for *_, h in gone), [g for g in gone if g[3] > 0.01])
    timeline = []
    for t, role, text, s, e in truths:
        if role == 'keep':
            at = next((c + (max(s, a) - a) for take, a, b, c in order if take == t and b > s and a < e), None)
            timeline.append((at, text, e - s))
    r.check('kept phrases play in the order they were said', [x[0] for x in timeline] == sorted(x[0] for x in timeline), timeline)
    pauses = [round(b[0] - (a[0] + a[2]), 2) for a, b in zip(timeline, timeline[1:])]
    slivers = [(take, round(b - a, 3)) for take, a, b, _ in order if b - a < 0.3]
    r.check('no sliver of a clip is left between the takes', not slivers, slivers)
    r.check('no long pause is left between sentences', all(p <= 0.45 for p in pauses), pauses)
    # Where the sound jumps: between takes, or where the cut skipped part of a take. Each side counts
    # only where something was cut away from it: a take that starts speaking at once is not cut there.
    length = {n: next(a['durationUs'] for a in state['assets'] if a['id'] == asset) / 1e6 for asset, n in assets.items()}
    cuts = [(b[3], a[2] < length[a[0]] - 0.001, b[1] > 0.001) for a, b in zip(order, order[1:])
            if a[0] != b[0] or abs(a[2] - b[1]) > 0.001]
    tracks = [t for t in state['tracks'] if t['kind'] == 'text']
    r.check('captions sit on one Captions track', len(tracks) == 1 and tracks[0]['name'] == 'Captions', [t['name'] for t in tracks])

    # One undo takes the whole run back and redo brings it again, captions included.
    r.s.run('document.activeElement?.blur()')
    clips = len(r.track())
    r.key('z', ctrlKey=True)
    r.check('one undo removes the whole run', wait(lambda: not r.track() and not r.state()['assets'], 10), r.track())
    r.key('z', ctrlKey=True, shiftKey=True)
    r.check('redo brings the whole run back', wait(lambda: len(r.track()) == clips and len(r.state()['tracks']) == 2, 10))
    caption = max(tracks[0]['clips'], key=lambda c: len(c['content']['text']))
    r.seek(caption['startUs'] + caption['durationUs'] // 2)
    time.sleep(1.5)
    r.shot('reel-preview')

    # The Reels preset in the export dialog, as the user picks it.
    r.key('e', ctrlKey=True)
    time.sleep(0.8)
    picked = r.s.run("""const b = [...document.querySelectorAll('[role=dialog] button')].find((b) => b.textContent.includes('Reels'));
        if (!b) return null; b.click(); return true;""")
    time.sleep(0.5)
    r.shot('export-dialog')
    dialog = r.s.run("return document.querySelector('[role=dialog]')?.textContent ?? ''")
    r.check('the export dialog offers the Reels & TikTok preset', picked and '1080' in dialog and '1920' in dialog, dialog[:300])
    r.key('Escape')

    out = r.work / 'reel.mp4'
    started = time.time()
    job = poll(bridge, bridge.call('export_video', {'path': str(out), 'preset': 'reels'}), 600)
    r.check('the export finishes', job['status'] == 'done', job)
    print(f'  export took {time.time() - started:.0f} s', flush=True)
    check_file(r, out, state['duration_us'] / 1e6, cuts)
    check_words(r, out, transcript, truths)
    check_captions(r, out, tracks[0]['clips'])
    r.check('no error toast', not r.errors(), r.errors())


def check_file(r, out, duration, cuts):
    info = probe(out)
    video = next(s for s in info['streams'] if s['codec_type'] == 'video')
    audio = next(s for s in info['streams'] if s['codec_type'] == 'audio')
    got = {k: video.get(k) for k in ('codec_name', 'profile', 'width', 'height', 'r_frame_rate', 'avg_frame_rate', 'pix_fmt',
                                     'color_space', 'color_transfer', 'color_primaries', 'color_range')}
    want = {'codec_name': 'h264', 'profile': 'High', 'width': 1080, 'height': 1920, 'r_frame_rate': '30/1',
            'avg_frame_rate': '30/1', 'pix_fmt': 'yuv420p', 'color_space': 'bt709', 'color_transfer': 'bt709',
            'color_primaries': 'bt709', 'color_range': 'tv'}
    r.check('video is H.264 High, 1080x1920, 30 fps, yuv420p, BT.709', got == want, got)
    sound = {k: audio.get(k) for k in ('codec_name', 'sample_rate', 'channels')}
    r.check('sound is AAC 48 kHz stereo', sound == {'codec_name': 'aac', 'sample_rate': '48000', 'channels': 2}, sound)
    boxes = atoms(out)
    r.check('moov comes before mdat, so the file starts playing at once', boxes.index('moov') < boxes.index('mdat'), boxes)
    length = float(info['format']['duration'])
    r.check('the file lasts as long as the timeline', abs(length - duration) < 0.1, [length, duration])
    lufs, peak = loudness(out)
    r.check('loudness is about -14 LUFS integrated', abs(lufs + 14) <= 1, lufs)
    r.check('true peak is at most -1 dBTP', peak <= -1.0, peak)
    # A cut in speech is a jump the ear hears: each side a cut took something from must be quiet there.
    speech = db(pcm(out, 0, duration))
    levels = [(c, round(db(pcm(out, c - 0.01, 0.01)), 1) if before else None, round(db(pcm(out, c, 0.01)), 1) if after else None)
              for c, before, after in cuts]
    r.check('every cut lies in silence', all(level is None or level < speech - 12 for _, *sides in levels for level in sides),
            {'cuts (time, before, after)': levels, 'speech': round(speech, 1)})


def check_words(r, out, transcript, truths):
    """The export heard again: the same words as the kept parts of the takes, each restart once, no filler."""
    exported = [w['text'] for w in recognise(out, r.work)]
    expected = [w['text'] for w in transcript['words']]
    heard_words, expected_words = words(' '.join(exported)), words(' '.join(expected))
    (r.work / 'export-words.txt').write_text(' '.join(exported) + '\n')
    joined = ' '.join(heard_words)
    r.check('the export says "Dneska vám ukážu" and "Kamera musí stát" once each',
            joined.count('dneska vam ukazu') == 1 and joined.count('kamera musi stat') == 1, ' '.join(exported))
    r.check('no filler is left', not {'ehm', 'jakoby', 'proste'} & set(heard_words), ' '.join(exported))
    kept = [w for t, role, text, s, e in truths if role == 'keep' for w in words(text)]
    ratio = difflib.SequenceMatcher(None, heard_words, kept).ratio()
    r.check('recognition of the export matches the kept sentences', ratio >= 0.85,
            {'ratio': round(ratio, 3), 'heard': ' '.join(exported)})


def check_captions(r, out, captions):
    """Exported frames in the middle of the longest captions: caption white only inside the safe area."""
    boxes = []
    for i, caption in enumerate(sorted(captions, key=lambda c: -len(c['content']['text']))[:4]):
        frame = r.work / f'export-caption-{i}.png'
        at = (caption['startUs'] + caption['durationUs'] / 2) / 1e6
        subprocess.run(['ffmpeg', '-v', 'error', '-y', '-ss', f'{at:.3f}', '-i', str(out), '-frames:v', '1', '-update', '1',
                        str(frame)], check=True)
        box, count = caption_box(frame)
        boxes.append({'text': caption['content']['text'], 'box': box, 'pixels': count})
        if i == 0:
            r.shots.append(frame.name)
    inside = all(b['box'] and b['pixels'] > 500 and SAFE[0] <= b['box'][0] and SAFE[1] <= b['box'][1]
                 and b['box'][2] <= SAFE[2] and b['box'][3] <= SAFE[3] for b in boxes)
    r.check('exported captions show inside the Reels safe area', inside, boxes)
