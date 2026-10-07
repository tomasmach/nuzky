import json, time

from e2e.harness import FIXTURES, flow, preview_brightness, wait


@flow('edit', 'Import two clips, split one with S, delete a piece with Delete, undo, and find the result saved')
def edit(r):
    r.import_media(FIXTURES / 'talk.mp4', FIXTURES / 'wide.mp4')
    for name in ('talk.mp4', 'wide.mp4'):
        r.add_clip(name)
    clips = r.track()
    r.check('clips sit back to back on the main track', clips[1]['startUs'] == clips[0]['startUs'] + clips[0]['durationUs'], clips)
    r.check('both clips get thumbnails', wait(lambda: len(r.state()['thumbs']) == 2), r.state()['thumbs'])
    r.seek(2_000_000)
    r.focus_clip(clips[0]['id'])
    r.key('s')
    split = wait(lambda: (c := r.track()) and len(c) == 3 and c)
    r.check('S splits the clip at the playhead', split and split[0]['durationUs'] == 2_000_000, split)
    piece = split[1]
    r.focus_clip(piece['id'])
    r.key('Delete')
    deleted = wait(lambda: (c := r.track()) and len(c) == 2 and c)
    end = lambda clips: clips[-1]['startUs'] + clips[-1]['durationUs']
    r.check('Delete removes the piece and closes the gap', deleted and end(deleted) == end(split) - piece['durationUs'], deleted)
    r.key('z', ctrlKey=True)
    undone = wait(lambda: (c := r.track()) and len(c) == 3 and c)
    r.check('Ctrl+Z brings the piece back', undone == split, undone)
    time.sleep(1.5)
    r.shot('timeline')
    # talk.mp4 is portrait and fills the 9:16 canvas; wide.mp4, which followed the deleted piece, leaves it black at the top.
    top = preview_brightness(r, r.work / 'timeline.png', 0.1)
    r.check('the preview shows the restored piece under the playhead', top > 40, round(top, 1))
    saved = lambda: len(json.loads(r.saved_project().read_text())['tracks'][0]['clips'])
    r.check('the project file on disk has the same three clips', wait(lambda: saved() == 3, 10), saved())
    r.check('no error toast', not r.errors(), r.errors())
