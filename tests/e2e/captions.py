import time

from e2e.harness import FIXTURES, changed_share, flow, link_models, preview_crop, preview_rect, preview_redraw, wait


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
    # Hiding and showing the captions track redraws the same moment, so the two pictures differ only by the caption.
    # Each is taken once the preview has redrawn, so a picture left from before the seek cannot pass for either.
    caption = max(track['clips'], key=lambda c: c['durationUs'])
    r.seek(caption['startUs'] + caption['durationUs'] // 2)
    time.sleep(1.5)
    r.shot('captions-seek')
    before = preview_crop(r.work / 'captions-seek.png', preview_rect(r))
    hide = "window.__capopen.store.getState().edit({type: 'updateTrack', trackId: arguments[0], hidden: arguments[1]})"
    r.s.call(hide, track['id'], True)
    hidden, redrawn_hidden = preview_redraw(r, 'captions-hidden', before)
    r.s.call(hide, track['id'], False)
    shown, redrawn_shown = preview_redraw(r, 'captions', hidden)
    r.check('the preview redraws after hiding and after showing the captions', redrawn_hidden and redrawn_shown,
            {'after hiding': redrawn_hidden, 'after showing': redrawn_shown})
    changed = changed_share(hidden, shown)
    r.check(f'the caption "{caption["text"]}" shows in the preview', changed > 0.005, f'{changed:.2%} of the preview changes')
    r.check('no error toast', not r.errors(), r.errors())
