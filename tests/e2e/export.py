import subprocess

from e2e.harness import CLI, FIXTURES, changed_share, close_window_and_confirm, ffprobe, flow, wait


@flow('export', 'Export refuses to overwrite, replaces only when asked, matches the rendered frame, and quitting mid-export cleans up')
def export(r):
    r.import_media(FIXTURES / 'talk.mp4')
    r.add_clip('talk.mp4')
    epoch = r.state()['epoch']
    target = r.work / 'existing.mp4'
    target.write_bytes(b'not a video')
    start_export = 'window.__nuzky.api.startExport(arguments[0], arguments[1], arguments[2], arguments[3])'
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
    r.check('no unfinished file is left', not list(r.work.glob('.nuzky-part-*')))
    # Closing the window during an export asks first, then removes the unfinished file.
    quit_target = r.work / 'quit.mp4'
    started = r.s.call(start_export, str(quit_target), {'resolution': 2160, 'fps': 60, 'quality': 'high'}, epoch, True)
    r.check('a long export runs into an unfinished file', started['ok'] and wait(lambda: list(r.work.glob('.nuzky-part-*')), 10))
    r.check('closing the window asks before quitting', close_window_and_confirm(r))
    gone = wait(lambda: not list(r.work.glob('.nuzky-part-*')) and not quit_target.exists(), 15)
    r.check('quitting removes the unfinished export', gone, [p.name for p in r.work.glob('.nuzky-part-*')])
