import subprocess
import time

from e2e.harness import CLI, changed_share, export, flow, preview_crop, preview_rect, wait


def phone_clip(path, seconds):
    """HEVC 10 bit HLG as a phone records it: stored landscape and shown portrait, at a jittered frame rate."""
    stored = path.with_suffix('.stored.mov')
    times = "settb=1/600,setpts='(N*20+eq(mod(N\\,4)\\,1)*7)/600/TB'"
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', f'testsrc2=s=2560x1440:r=30:d={seconds}', '-vf',
                    f'{times},scale=out_color_matrix=bt2020nc:out_range=tv,format=yuv420p10le', '-fps_mode', 'vfr',
                    '-enc_time_base', '1/600', '-c:v', 'libx265', '-preset', 'ultrafast', '-x265-params',
                    'log-level=error:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc', '-tag:v', 'hvc1',
                    '-video_track_timescale', '600', str(stored)], check=True)
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-display_rotation', '-90', '-i', str(stored), '-c', 'copy', str(path)],
                   check=True)


def job(r, status):
    return next((j for j in r.state()['jobs'] if j['kind'] == 'proxy' and j['status'] == status), None)


def copies(r):
    return sorted((r.work / 'cache/nuzky/proxy').glob('*.mp4'))


def rendered(r, seconds, width):
    """The frame the renderer draws from the original file, as export does."""
    png = r.work / f'original-{seconds}.png'
    subprocess.run([str(CLI), 'frame', str(r.saved_project()), str(seconds), str(png), str(width)], env=r.env, check=True,
                   capture_output=True)
    return png


def shown(r, name, size):
    """The preview, screenshotted and scaled to `size`."""
    r.shot(name)
    return preview_crop(r.work / f'{name}.png', preview_rect(r)).resize(size)


@flow('proxy', 'Phone HEVC previews from a light copy made in the background, which Stop stops; export reads the original')
def proxy(r):
    clip, long_clip = r.work / 'phone.mov', r.work / 'phone-long.mov'
    phone_clip(clip, 3)
    phone_clip(long_clip, 12)
    r.import_media(clip)
    r.add_clip(clip.name)
    done = wait(lambda: job(r, 'done'), 120)
    r.check('a preview copy of the phone clip is made in the background', done and len(copies(r)) == 1,
            [p.name for p in copies(r)])

    # The preview, now from the copy, shows the frame the renderer draws from the original.
    r.seek(1_500_000)
    time.sleep(1.5)
    changed = changed_share(shown(r, 'preview', (360, 640)), rendered(r, 1.5, 360))
    r.check('the preview from the copy shows the original frame, upright and in its colours', changed < 0.02,
            f'{changed:.2%} of pixels differ')

    # A swapped-in magenta copy shows in the preview, so the preview reads the copy; export still shows the file.
    magenta = r.work / 'magenta.mp4'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', 'color=magenta:s=192x108:r=30:d=4', '-c:v',
                    'libx264', str(magenta)], check=True)
    magenta.replace(copies(r)[0])
    # The preview lets a decoder go after 5 s unused, so the next frame opens the swapped copy.
    time.sleep(6)
    r.seek(2_000_000)
    time.sleep(1.5)
    centre = shown(r, 'swapped-copy', (360, 640)).getpixel((180, 320))
    r.check('the preview reads the copy', centre[0] > 200 and centre[1] < 60 and centre[2] > 200, centre)
    target = export(r, 'export.mp4')
    exported = r.work / 'export-frame.png'
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', str(target), '-vf', 'select=eq(n\\,60)', '-frames:v', '1',
                    '-update', '1', str(exported)], check=True)
    changed = changed_share(exported, rendered(r, 2, 720))
    r.check('export reads the original, not the copy', changed < 0.002, f'{changed:.3%} of pixels differ')

    # Stop ends a running preparation, leaves nothing behind and is not undone by the next edit.
    r.import_media(long_clip)
    running = wait(lambda: job(r, 'running'), 30, 0.05)
    r.check('the next phone clip gets its copy prepared', running, r.state()['jobs'])
    r.shot('preparing')
    r.s.run("[...document.querySelectorAll('[role=status] button')].find((b) => b.textContent.trim() === 'Stop').click()")
    r.check('Stop ends the preparation within seconds', wait(lambda: job(r, 'cancelled'), 5), r.state()['jobs'])
    leftovers = [p.name for p in (r.work / 'cache/nuzky/proxy').iterdir() if not p.name.endswith('.lock')]
    r.check('a stopped preparation leaves no file behind', len(leftovers) == 1, leftovers)
    r.add_clip(long_clip.name)
    time.sleep(3)
    r.check('the next edit does not start the stopped preparation again', not job(r, 'running'), r.state()['jobs'])
    r.check('nothing shows an error', not r.errors(), r.errors())
