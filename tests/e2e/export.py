import json
import subprocess
import time

from e2e.harness import CLI, FIXTURES, changed_share, close_window_and_confirm, ffprobe, flow, wait


@flow('export', 'Export refuses to overwrite, replaces only when asked, matches the rendered frame, offers the presets that fit '
      'a 16:9 video and writes YouTube 1080p, and quitting mid-export cleans up')
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
    youtube(r, start_export)
    # Closing the window during an export asks first, then removes the unfinished file.
    quit_target = r.work / 'quit.mp4'
    started = r.s.call(start_export, str(quit_target), {'resolution': 2160, 'fps': 60, 'quality': 'high'}, epoch, True)
    r.check('a long export runs into an unfinished file', started['ok'] and wait(lambda: list(r.work.glob('.nuzky-part-*')), 10))
    r.check('closing the window asks before quitting', close_window_and_confirm(r))
    gone = wait(lambda: not list(r.work.glob('.nuzky-part-*')) and not quit_target.exists(), 15)
    r.check('quitting removes the unfinished export', gone, [p.name for p in r.work.glob('.nuzky-part-*')])


DIALOG = "return document.querySelector('[role=dialog]')?.textContent ?? ''"
PRESETS = """return [...document.querySelectorAll('[role=menu][aria-label=Preset] [role=menuitemradio]')]
  .map((i) => ({text: i.textContent, disabled: i.getAttribute('aria-disabled') === 'true', reason: i.title}))"""
PICK = """[...document.querySelectorAll('[role=menu][aria-label=Preset] [role=menuitemradio]')]
  .find((i) => i.textContent.startsWith(arguments[0])).click()"""
PRESSED = """return document.querySelector(`[role=dialog] [aria-label='${arguments[0]}'] [aria-pressed=true]`)?.textContent"""


def open_presets(r):
    r.s.run("document.querySelector('[role=dialog] button[aria-haspopup=menu]').click()")
    return wait(lambda: r.s.run(PRESETS), 5)


def youtube(r, start_export):
    """A 16:9 video: the YouTube presets fit it, the others stay listed but cannot be picked, and YouTube 1080p
    writes 1920x1080 at the frame rate chosen in the dialog with the sound at -14 LUFS."""
    edited = r.s.call("window.__nuzky.store.getState().edit({type: 'setCanvas', width: 1920, height: 1080})")
    r.check('the canvas turns 16:9', edited['ok'] and wait(
        lambda: r.s.run('return window.__nuzky.store.getState().snap.project.canvas.width') == 1920, 10), edited)
    r.key('e', ctrlKey=True)
    r.check('Ctrl+E opens the export dialog', wait(lambda: r.s.run(DIALOG), 5))
    time.sleep(0.5)  # the dialog fades in over 200 ms
    items = open_presets(r)
    usable = [i['text'] for i in items if not i['disabled']]
    r.check('the preset menu offers Custom and the YouTube presets first, with their sizes',
            usable == ['Custom', 'YouTube 1080p1920×1080', 'YouTube 4K3840×2160'], usable)
    blocked = {i['text']: i['reason'] for i in items if i['disabled']}
    r.check('the presets for other formats are listed after them, disabled, each saying what it needs',
            [i['disabled'] for i in items] == [False] * 3 + [True] * 4
            and blocked.get('Reels & TikTokNeeds 9:16') == 'Reels & TikTok needs a 9:16 video. Switch Ratio under the preview to 9:16.'
            and 'Instagram feedNeeds 4:5' in blocked and 'SquareNeeds 1:1' in blocked and 'YouTube ShortsNeeds 9:16' in blocked,
            blocked)
    time.sleep(0.3)
    r.shot('export-presets-16x9')
    r.key('Escape')
    r.check('Esc closes the preset menu and keeps the dialog', wait(lambda: not r.s.run(PRESETS), 5) and r.s.run(DIALOG))
    open_presets(r)
    r.s.run(PICK, 'YouTube 1080p')
    # YouTube keeps the frame rate free: another one stays YouTube 1080p.
    r.s.run("[...document.querySelectorAll(\"[role=dialog] [aria-label='Frame rate'] button\")].find((b) => b.textContent === '25').click()")
    time.sleep(0.3)
    # .click() carries no pointer, as Enter on the button: the menu opens on the current preset, so Enter keeps it.
    open_presets(r)
    active = r.s.run("const m = document.querySelector('[role=menu][aria-label=Preset]');"
                     "return document.getElementById(m.getAttribute('aria-activedescendant'))?.textContent ?? null")
    r.check('the preset menu opened from the keyboard starts on the current preset', (active or '').startswith('YouTube 1080p'), active)
    r.s.run("document.querySelector('[data-scrim]').dispatchEvent(new PointerEvent('pointerdown', {bubbles: true}))")
    r.check('a click beside the menu closes only the menu', wait(lambda: not r.s.run(PRESETS), 5) and r.s.run(DIALOG))
    dialog = r.s.run(DIALOG)
    options = {'resolution': int(r.s.run(PRESSED, 'Resolution').rstrip('p').replace('4K', '2160')),
               'fps': int(r.s.run(PRESSED, 'Frame rate')), 'quality': 'recommended',
               'preset': json.loads(r.s.run("return localStorage.getItem('nuzky.export')"))['preset']}
    r.check('YouTube 1080p sets 1920x1080 with -14 LUFS and keeps 25 fps, ready to export',
            'YouTube 1080p' in dialog and '1920×1080' in dialog and 'Loudness −14 LUFS' in dialog
            and options == {'resolution': 1080, 'fps': 25, 'quality': 'recommended', 'preset': 'youtube_1080p'}
            and r.s.run("return [...document.querySelectorAll('[role=dialog] button')].find((b) => b.textContent === 'Export…')"
                        ".getAttribute('aria-disabled') !== 'true'"), [options, dialog[:300]])
    r.shot('export-youtube-1080p')
    r.key('Escape')
    r.check('Esc then closes the dialog', wait(lambda: not r.s.run(DIALOG), 5))

    epoch = r.state()['epoch']
    reel = r.work / 'wide-reel.mp4'
    refused = r.s.call(start_export, str(reel), {'resolution': 1080, 'fps': 30, 'quality': 'small', 'preset': 'reels'}, epoch, True)
    r.check('the engine refuses Reels for a 16:9 video', not refused['ok'] and 'needs a 9:16 video' in refused['error']
            and not reel.exists(), refused)
    target = r.work / 'youtube.mp4'
    started = r.s.call(start_export, str(target), options, epoch, True)
    job = started['ok'] and wait(lambda: (j := r.s.run("return window.__nuzky.store.getState().jobs[arguments[0]] ?? null", started['value']))
                                 and j['status'] != 'running' and j, 300)
    r.check('the YouTube 1080p export finishes', job and job['status'] == 'done', [started, job])
    probe = json.loads(subprocess.run(['ffprobe', '-v', 'error', '-select_streams', 'v', '-show_entries',
                                       'stream=width,height,r_frame_rate,profile', '-of', 'json', str(target)],
                                      capture_output=True, text=True, check=True).stdout)['streams'][0]
    r.check('the file is 1920x1080 H.264 High at 25 fps', probe == {'width': 1920, 'height': 1080, 'profile': 'High',
                                                                    'r_frame_rate': '25/1'}, probe)
    log = subprocess.run(['ffmpeg', '-nostdin', '-hide_banner', '-i', str(target), '-vn', '-af', 'ebur128=peak=true', '-f', 'null', '-'],
                         capture_output=True, text=True).stderr
    summary = log[log.rfind('Summary:'):].split()
    lufs, peak = float(summary[summary.index('I:') + 1]), float(summary[summary.index('Peak:') + 1])
    r.check('the sound is levelled to -14 LUFS with true peak at most -1 dBTP', abs(lufs + 14) <= 1 and peak <= -1, [lufs, peak])
