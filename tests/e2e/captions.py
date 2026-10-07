import time

from e2e.harness import FIXTURES, changed_share, flow, link_models, preview_crop, preview_rect, wait


@flow('captions', 'Whisper on this computer captions the spoken words of a video', before=link_models)
def captions(r):
    r.import_media(FIXTURES / 'talk.mp4')
    r.add_clip('talk.mp4')
    r.s.run("window.__capopen.speech.setState({model: 'small', language: 'en'})")
    r.s.run("[...document.querySelectorAll('[role=tab]')].find((t) => t.textContent.trim() === 'Captions').click()")
    time.sleep(0.5)
    r.s.run("[...document.querySelectorAll('button')].find((b) => b.textContent.includes('Generate captions')).click()")
    job = wait(lambda: next((j for j in r.state()['jobs'] if j['kind'] == 'captions' and j['status'] != 'running'), None), 300, 1)
    r.check('captions finish', job and job['status'] == 'done', job)
    track = next(t for t in r.state()['tracks'] if t['kind'] == 'text')
    texts = [c['text'] for c in track['clips']]
    heard = ' '.join(texts).lower()
    spoken = ['channel', 'video', 'laptop', 'clips', 'pauses', 'captions']
    found = [w for w in spoken if w in heard]
    r.check('captions contain the spoken words', len(found) >= 4, {'found': found, 'captions': texts[:8]})
    # The same moment with the captions track shown and hidden differs only by the caption itself.
    caption = max(track['clips'], key=lambda c: c['durationUs'])
    r.seek(caption['startUs'] + caption['durationUs'] // 2)
    time.sleep(1.5)
    r.shot('captions')
    hide = "window.__capopen.store.getState().edit({type: 'updateTrack', trackId: arguments[0], hidden: arguments[1]})"
    r.s.call(hide, track['id'], True)
    time.sleep(1.5)
    r.shot('captions-hidden')
    r.s.call(hide, track['id'], False)
    rect = preview_rect(r)
    changed = changed_share(preview_crop(r.work / 'captions.png', rect), preview_crop(r.work / 'captions-hidden.png', rect))
    r.check(f'the caption "{caption["text"]}" shows in the preview', changed > 0.005, f'{changed:.2%} of the preview changes')
    r.check('no error toast', not r.errors(), r.errors())
