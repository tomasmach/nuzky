"""Cover frames. The app knows which face and subject models are installed and checks or downloads them as a job;
an agent attached to the open app picks cover frames of the timeline and masks the person with them. The models
are linked from tmp-test, so nothing is downloaded. Without ONNX Runtime the app still opens and edits, and covers
say they cannot run instead of offering a download."""
import json, os, shutil, subprocess, time

from e2e.harness import CLI, FIXTURES, MODELS, Bridge, flow, wait

VISION = ('yunet-2026may.onnx', 'face-landmarks-v2.onnx', 'face-blendshapes-v2.onnx', 'selfie-segmenter.onnx',
          'birefnet-lite.onnx')
INVOKE = 'window.__TAURI_INTERNALS__.invoke(arguments[0]).then((v) => JSON.stringify(v))'


def face_project(r):
    project = r.work / 'data/nuzky/projects/face.nuzky'
    subprocess.run([str(CLI), 'new', str(project), str(FIXTURES / 'face-thumb.mp4')], env=r.env, check=True,
                   capture_output=True)


def invoke(r, command):
    answer = r.s.call(INVOKE, command)
    if not answer['ok']:
        raise RuntimeError(f'{command}: {answer}')
    return json.loads(answer['value'])


def poll(bridge, job, timeout):
    done = wait(lambda: (s := bridge.call('job', {'job_id': job['job_id'], 'action': 'get'}))['status'] != 'running' and s,
                timeout, 0.5)
    return done or {'status': 'timed out'}


@flow('covers', 'Cover models: what is missing, the checking job, and cover frames and a person mask through the app',
      before=face_project)
def covers(r):
    status = invoke(r, 'vision_models')
    r.check('without models the app says what the download is', status == {'sizeMb': 196, 'downloaded': False, 'unavailable': None}, status)
    models = r.work / 'data/nuzky/models'
    models.mkdir(parents=True, exist_ok=True)
    for name in VISION:
        try:
            os.link(MODELS / name, models / name)
        except OSError:
            shutil.copy(MODELS / name, models / name)
    status = invoke(r, 'vision_models')
    r.check('with the models in place nothing is left to download', status == {'sizeMb': 0, 'downloaded': True, 'unavailable': None}, status)
    invoke(r, 'start_vision_models')
    job = wait(lambda: (j := next((j for j in r.state()['jobs'] if j['kind'] == 'vision-models'), None)) and j['status'] != 'running' and j, 60)
    r.check('the model job checks the installed models and finishes', job and job['status'] == 'done', job)

    bridge = Bridge(r, r.saved_project())
    try:
        started = time.monotonic()
        frames = poll(bridge, bridge.call('analyze', {'kind': 'thumbnail_frames'}), 120)
        seconds = time.monotonic() - started
        candidates = frames.get('result', {}).get('candidates', [])
        best = candidates[0] if candidates else {}
        # The fixture: [0,3) s blurred, [3,6) a blink, [6,12) sharp open eyes.
        r.check('the cover frame has sharp open eyes', frames['status'] == 'done' and best.get('time_us', 0) >= 6_000_000
                and best['parts']['eyes_open'] > 0.9, {'seconds': round(seconds, 1), 'best': best})
        # The person model ran in the app under the system's locale: with a decimal comma it once saw nobody.
        box = best.get('subject_box')
        r.check('the cover frame knows where the person is', box and box[2] * box[3] > 0.2 * 1080 * 1920, box)
        r.seek(best.get('time_us', 0))
        time.sleep(1)
        r.shot('cover-frame')
        mask = poll(bridge, bridge.call('segment_subject', {'time_us': best.get('time_us', 7_500_000)}), 120)
        result = mask.get('result') or {}
        r.check('the mask covers the person', mask['status'] == 'done' and result.get('person')
                and 0.6 < result.get('subject_share', 0) < 0.8, mask)
    finally:
        bridge.close()
    r.check('no error toast', not r.errors(), r.errors())


def no_runtime(r):
    face_project(r)
    r.env['NUZKY_ONNXRUNTIME'] = str(r.work / 'missing/libonnxruntime')


@flow('covers_unavailable', 'Without ONNX Runtime the app opens and edits, and covers say they cannot run',
      before=no_runtime)
def covers_unavailable(r):
    status = invoke(r, 'vision_models')
    reason = status.get('unavailable') or ''
    r.check('covers say they cannot run and why', reason.startswith('VISION_UNAVAILABLE') and 'missing' in reason, status)
    refused = r.s.call("window.__TAURI_INTERNALS__.invoke('start_vision_models')")
    r.check('nothing is downloaded for them', not refused['ok'] and 'VISION_UNAVAILABLE' in refused['error'], refused)
    bridge = Bridge(r, r.saved_project())
    try:
        answer = bridge.rpc('tools/call', {'name': 'analyze', 'arguments': {'kind': 'thumbnail_frames'}})
        text = answer['result']['content'][0]['text']
        r.check('an agent is told covers cannot run here', answer['result'].get('isError') and 'VISION_UNAVAILABLE' in text, text)
    finally:
        bridge.close()
    # In the app: Pick for me says why it is off; choosing a frame and text in front still work.
    r.s.run("""const b = [...document.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === 'Make a cover'); b.click();""")
    pick = wait(lambda: r.s.run("""const b = [...document.querySelectorAll('[data-testid=cover-frames] button')].find((b) => b.textContent.trim() === 'Pick for me');
        return b ? {disabled: b.getAttribute('aria-disabled'), title: b.title} : null;"""), 10)
    r.check('Pick for me is off with the reason', pick and pick['disabled'] == 'true' and "doesn't work on this computer" in pick['title'], pick)
    r.shot('cover-unavailable')
    r.key('Escape')
    clip = r.track()[0]
    r.focus_clip(clip['id'])
    r.key('Delete')
    r.check('editing still works', wait(lambda: not r.track(), 10), r.track())
    r.shot('covers-unavailable')
    r.check('no error toast', not r.errors(), r.errors())
