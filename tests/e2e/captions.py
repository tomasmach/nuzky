import time

from e2e.harness import FIXTURES, flow, link_models, wait


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
    texts = [c['text'] for t in r.state()['tracks'] if t['kind'] == 'text' for c in t['clips'] if c['text']]
    heard = ' '.join(texts).lower()
    spoken = ['channel', 'video', 'laptop', 'clips', 'pauses', 'captions']
    found = [w for w in spoken if w in heard]
    r.check('captions contain the spoken words', len(found) >= 4, {'found': found, 'captions': texts[:8]})
    r.seek(2_000_000)
    time.sleep(1.5)
    r.shot('captions')
    r.check('no error toast', not r.errors(), r.errors())
