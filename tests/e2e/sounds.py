"""The sound library in the Audio panel. Music and Sound effects search Openverse, served here in its place with
licences it should never return, so only CC0 and CC BY may show; a sound plays before it is added, goes to the
playhead as one undo step and remembers its author and licence; Copy credits and the file beside the export credit
the CC BY ones; the built-in effects need no network, and offline the panel says so. An agent finds and adds sounds
through the same library."""
import json, subprocess, threading, time, urllib.parse
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from e2e.harness import FIXTURES, Bridge, export, ffprobe, flow, press, wait
from e2e.layout import resize

RAINY = 'aaaaaaaa-0000-4000-8000-000000000001'
LANTERNS = 'aaaaaaaa-0000-4000-8000-000000000002'
NC = 'aaaaaaaa-0000-4000-8000-000000000003'
ND = 'aaaaaaaa-0000-4000-8000-000000000004'
SA = 'aaaaaaaa-0000-4000-8000-000000000005'
JAMENDO = 'aaaaaaaa-0000-4000-8000-000000000007'
# Found only by id, its file answers after 15 s: a download the user stops.
SLOW = 'aaaaaaaa-0000-4000-8000-000000000006'
DEED = {'by': 'https://creativecommons.org/licenses/{}/4.0/', 'cc0': 'https://creativecommons.org/publicdomain/zero/1.0/'}

TEXTS = """return [...document.querySelectorAll('[data-sound]')].map((row) => ({id: row.dataset.sound,
  text: row.innerText.replace(/\\s+/g, ' ').trim()}));"""
TYPE = """const input = document.querySelector(`input[aria-label="${arguments[0]}"]`); input.focus();
Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, arguments[1]);
input.dispatchEvent(new Event('input', {bubbles: true}));
input.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true}));"""
CLICK = "const el = document.querySelector(arguments[0]); if (!el) return false; el.click(); return true;"
SOUNDS = "const s = window.__nuzky.sounds.getState(); return {preview: s.preview, adding: s.adding, search: s.search};"
CREDITED = """return window.__nuzky.store.getState().snap.project.assets.map((a) => ({id: a.id, name: a.name, path: a.path,
  credit: a.credit ?? null}));"""


def audio(id, title, creator, license, file, source='jamendo', category='music'):
    return {'id': id, 'title': title, 'creator': creator, 'license': license, 'license_version': '4.0' if license != 'cc0' else '1.0',
            'license_url': DEED['cc0'] if license == 'cc0' else DEED['by'].format(license),
            'foreign_landing_url': f'https://example.org/{file}', 'url': None, 'duration': None, 'source': source,
            'category': category, 'file': file}


class Openverse:
    """Openverse's audio search and its files. It answers every search with the same page, NC, ND and SA among it,
    as a catalogue with a wrong filter would."""

    def __init__(self, work):
        self.requests, self.files = [], {}
        for name, seconds, hz in (('rainy.mp3', 6, 440), ('lanterns.mp3', 3, 660), ('slow.mp3', 3, 330)):
            path = work / name
            subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'lavfi', '-i', f'sine=frequency={hz}:duration={seconds}',
                            '-ac', '2', '-c:a', 'libmp3lame', '-b:a', '128k', str(path)], check=True)
            self.files[name] = path.read_bytes()
        server = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                server.requests.append(self.path)
                url = urllib.parse.urlparse(self.path)
                if url.path.startswith('/files/') and url.path[7:] in server.files:
                    if url.path == '/files/slow.mp3':
                        time.sleep(15)
                    return self.reply(server.files[url.path[7:]], 'audio/mpeg')
                if url.path == f'/v1/audio/{SLOW}/':
                    slow = audio(SLOW, 'Slow Song', 'Slow Band', 'by', 'slow.mp3', source='freesound')
                    slow['url'], slow['duration'] = f"{server.base}/files/{slow.pop('file')}", 3000
                    return self.reply(json.dumps(slow).encode())
                if url.path == '/v1/audio/':
                    return self.reply(json.dumps({'result_count': 5, 'page_count': 1, 'page': 1, 'results': server.results()}).encode())
                found = [a for a in server.results() if url.path == f"/v1/audio/{a['id']}/"]
                if found:
                    return self.reply(json.dumps(found[0]).encode())
                self.send_response(404)
                self.end_headers()

            def reply(self, body, kind='application/json'):
                self.send_response(200)
                self.send_header('Content-Type', kind)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        self.http = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=self.http.serve_forever, daemon=True).start()
        self.base = f'http://127.0.0.1:{self.http.server_port}'

    def results(self):
        found = [audio(RAINY, 'Rainy Window', 'Lumen Drift', 'by', 'rainy.mp3', source='wikimedia_audio'),
                 audio(JAMENDO, 'Jamendo Track', 'Somebody', 'by', 'rainy.mp3'),
                 audio(NC, 'Not For Profit', 'Nobody', 'by-nc', 'rainy.mp3'),
                 audio(ND, 'No Changes', 'Nobody', 'by-nd', 'rainy.mp3'),
                 audio(SA, 'Share Alike', 'Nobody', 'by-sa', 'rainy.mp3'),
                 audio(LANTERNS, 'Paper Lanterns', 'Teodor Ash', 'cc0', 'lanterns.mp3', source='freesound', category=None)]
        for a in found:
            a['url'], a['duration'] = f"{self.base}/files/{a.pop('file')}", 6000 if a['id'] == RAINY else 3000
        return found

    def stop(self):
        self.http.shutdown()
        self.http.server_close()


def serve(r):
    r.openverse = Openverse(r.work)
    r.env['NUZKY_OPENVERSE_URL'] = r.openverse.base + '/v1/'


def rows(r):
    return r.s.run(TEXTS)


def clips_of(r, asset_id):
    return [(t['id'], c['startUs']) for t in r.state()['tracks'] for c in t['clips'] if c['assetId'] == asset_id]


def shot(r, name):
    # Headless WebKit can take a moment to finish a colour transition.
    time.sleep(0.4)
    r.shot(name)


def library_assets(r):
    return [a for a in r.s.run(CREDITED) if a['credit']]


@flow('sounds', __doc__.split('.')[0], before=serve)
def sounds(r):
    # A video on the main track, so the playhead can move.
    r.import_media(FIXTURES / 'talk.mp4')
    r.add_clip('talk.mp4')
    r.s.run("window.__nuzky.store.setState({panelTab: 'audio'})")
    r.check('the Audio panel opens on In project, as before', wait(lambda: r.s.run("return !!document.querySelector('#audio-tab-project[aria-selected=true]')"), 5))
    r.s.run(CLICK, '#audio-tab-music')
    r.check('Music offers ideas and sends nothing before the user searches',
            wait(lambda: r.s.run("return [...document.querySelectorAll('button')].some((b) => b.textContent === 'Lofi')"), 5)
            and r.openverse.requests == [], r.openverse.requests)
    shot(r, 'music-empty')

    # Search: one request with the text and the licence filter, and only CC0 and CC BY come back.
    r.s.run("[...document.querySelectorAll('button')].find((b) => b.textContent === 'Lofi').click()")
    found = wait(lambda: len(rows(r)) >= 2 and rows(r), 10) or rows(r)
    titles = ' | '.join(x['text'] for x in found)
    r.check('Rainy Window (CC BY) and Paper Lanterns (CC0) show with author, source and licence',
            any('Rainy Window' in x['text'] and 'Lumen Drift · Wikimedia Commons' in x['text'] and 'CC BY' in x['text'] for x in found)
            and any('Paper Lanterns' in x['text'] and 'CC0' in x['text'] for x in found), titles)
    r.check('NC, ND and SA never show, whatever the catalogue sends', not any(t in titles for t in ('Not For Profit', 'No Changes', 'Share Alike')), titles)
    r.check("Jamendo's tracks stay out", 'Jamendo Track' not in titles, titles)
    query = urllib.parse.parse_qs(urllib.parse.urlparse(r.openverse.requests[0]).query)
    r.check('the request carries the search text and the licence filter, nothing from the project',
            query.get('q') == ['Lofi music'] and query.get('license') == ['cc0,by'] and query.get('excluded_source') == ['jamendo']
            and set(query) <= {'q', 'license', 'page', 'page_size', 'mature', 'filter_dead', 'excluded_source'}, query)
    r.check('Openverse is named under the results, with the advice to check the licence',
            r.s.run("return document.body.innerText.includes('Search uses Openverse. Nuzky is not endorsed or certified by Openverse.')"))

    # Preview: the file downloads once and plays through to its end; nothing is added.
    r.s.run(CLICK, f'[data-sound="openverse:{RAINY}"] button[aria-label^="Play"]')
    playing = wait(lambda: (p := r.s.run(SOUNDS)['preview']) and not p['loading'], 15)
    r.check('Play downloads Rainy Window and plays it', playing and '/files/rainy.mp3' in r.openverse.requests, r.openverse.requests)
    shot(r, 'music-preview')
    r.check('the preview stops by itself at the end of the sound', wait(lambda: r.s.run(SOUNDS)['preview'] is None, 12))
    r.check('previewing adds nothing to the project', library_assets(r) == [])

    # Add at the playhead: one undo step, with the credit and the file in Nuzky's library.
    r.seek(2_000_000)
    wait(lambda: r.s.run('return window.__nuzky.store.getState().timeUs') == 2_000_000, 5)
    r.s.run(CLICK, f'[data-sound="openverse:{RAINY}"] button[aria-label^="Add"]')
    added = wait(lambda: library_assets(r), 15) or []
    rainy = next((a for a in added if a['credit'] and a['credit']['id'] == RAINY), None)
    r.check('+ adds Rainy Window with its author, licence and page',
            rainy and rainy['credit'] == {'source': 'openverse', 'id': RAINY, 'title': 'Rainy Window', 'author': 'Lumen Drift',
                                         'license': 'by', 'licenseVersion': '4.0', 'licenseUrl': DEED['by'].format('by'),
                                         'url': 'https://example.org/rainy.mp3'}, added)
    r.check('its file is kept in the sound library, not in the cache', rainy and rainy['path'].startswith(str(r.work / 'data/nuzky/sounds/')), rainy)
    placed = clips_of(r, rainy['id']) if rainy else []
    r.check('the clip starts at the playhead on an audio track', len(placed) == 1 and placed[0][1] == 2_000_000 and placed[0][0] != 'main', placed)
    r.check('the file was downloaded only once, for the preview', r.openverse.requests.count('/files/rainy.mp3') == 1, r.openverse.requests)
    r.s.call('window.__nuzky.store.getState().undo()')
    r.check('one Undo takes the sound and its clip away', wait(lambda: library_assets(r) == [] and len(r.state()['assets']) == 1, 5))
    r.s.call('window.__nuzky.store.getState().redo()')
    r.check('Redo brings them back', wait(lambda: len(library_assets(r)) == 1 and len(clips_of(r, rainy['id'])) == 1, 5))
    r.check('the row shows the sound is in the project', wait(lambda: r.s.run(f"""return !!document.querySelector('[data-sound="openverse:{RAINY}"] [title="In this project"]')"""), 5))
    r.check('the panel says one sound needs credit', wait(lambda: r.s.run("return document.body.innerText.includes('1 sound needs credit')"), 5))
    shot(r, 'music-added')

    # Credits: the same text the export writes, copied for the description.
    credits = r.s.call("window.__nuzky.api.soundCredits(window.__nuzky.store.getState().snap.sessionEpoch)")
    r.check('the credits name the song, its author, page and licence, and say it is synced to the video',
            credits['ok'] and '"Rainy Window" by Lumen Drift (https://example.org/rainy.mp3), licensed under CC BY 4.0 '
            '(https://creativecommons.org/licenses/by/4.0/). Synced to video' in (credits['value'] or ''), credits)
    r.s.run("window.__nuzky.store.setState({toasts: []})")
    # A real key press, as the clipboard takes text only from something the user did.
    r.s.run("[...document.querySelectorAll('button')].find((b) => b.textContent.includes('Copy credits')).focus()")
    press('Return')
    toast = wait(lambda: r.state()['toasts'], 5) or []
    r.check('Copy credits copies them and says where they go', toast and toast[0]['kind'] == 'success' and 'description' in toast[0]['text'], toast)

    # An agent finds and adds a sound through the same library, inside the app.
    bridge = Bridge(r, r.saved_project())
    try:
        found = bridge.call('search_sounds', {'query': 'lantern', 'kind': 'music'})
        ids = [s['id'] for s in found['sounds']]
        r.check('search_sounds finds the same two sounds for an agent, without NC, ND or SA',
                ids == [f'openverse:{RAINY}', f'openverse:{LANTERNS}'], ids)
        run = bridge.call('begin_run', {'label': 'Add music'})['run_id']
        agent = bridge.call('add_sound', {'run_id': run, 'id': f'openverse:{LANTERNS}', 'at_us': 8_000_000})
        bridge.call('end_run', {'run_id': run, 'action': 'keep'})
        r.check('add_sound places Paper Lanterns at 8 s and says it needs no credit',
                agent['start_us'] == 8_000_000 and agent['needs_credit'] is False, agent)
        r.check('the app shows the agent\'s sound at once', wait(lambda: len(library_assets(r)) == 2, 5), r.state()['assets'])

        # Stop in the app ends an agent's download within seconds, even while the server keeps silent.
        run = bridge.call('begin_run', {'label': 'Add a slow song'})['run_id']
        outcome = {}

        def slow_add():
            try:
                outcome['result'] = bridge.call('add_sound', {'run_id': run, 'id': f'openverse:{SLOW}', 'at_us': 0})
            except Exception as e:
                outcome['error'] = str(e)
        adding = threading.Thread(target=slow_add)
        adding.start()
        wait(lambda: '/files/slow.mp3' in r.openverse.requests, 10)
        stopped_at = time.time()
        r.s.call("window.__nuzky.api.stopRun(window.__nuzky.store.getState().snap.sessionEpoch)"
                 ".then((s) => window.__nuzky.store.getState().setSnap(s))")
        adding.join(10)
        took = time.time() - stopped_at
        r.check('Stop ends the agent\'s download within seconds and adds nothing',
                'CANCELLED' in outcome.get('error', '') and took < 3 and len(library_assets(r)) == 2, (outcome, round(took, 1)))
        kept = [p.name for p in Path(r.work / 'data/nuzky/sounds').glob('*slow*')] + [p.name for p in Path(r.work / 'cache').rglob('*000006*')]
        r.check('no part of the stopped download is left behind', kept == [], kept)
    finally:
        bridge.close()

    # Built-in effects need no network.
    before = len(r.openverse.requests)
    r.s.run(CLICK, '#audio-tab-effects')
    effects = wait(lambda: len(rows(r)) >= 40 and rows(r), 5) or []
    r.check('Sound effects lists the built-in sounds', len(effects) >= 40, len(effects))
    r.s.run("[...document.querySelectorAll('[aria-label=\"Kinds of sound effects\"] button')].find((b) => b.textContent === 'Whoosh').click()")
    whooshes = wait(lambda: (x := rows(r)) and len(x) < 10 and x, 5) or []
    kinds = r.s.run("return Object.fromEntries(window.__nuzky.sounds.getState().builtIn.map((s) => [s.id, s.category]))")
    r.check('the Whoosh group shows only whooshes and swishes', whooshes and all(kinds[x['id']] == 'Whoosh' for x in whooshes), whooshes)
    shot(r, 'effects')
    r.seek(0)
    r.s.run(CLICK, '[data-sound="nuzky:whoosh"] button[aria-label^="Add"]')
    r.check('+ adds the built-in Whoosh at the playhead', wait(lambda: len(library_assets(r)) == 3, 10), r.state()['assets'])
    r.check('built-in sounds reach no server', len(r.openverse.requests) == before, r.openverse.requests[before:])

    # The export carries the credits beside it; the built-in and CC0 sounds need none.
    video = export(r, 'reel.mp4')
    beside = video.with_name('reel.credits.txt')
    text = beside.read_text() if beside.exists() else ''
    r.check('the export writes reel.credits.txt with the CC BY song and the CC0 one as a courtesy',
            text.startswith('Music and sound effects:\n"Rainy Window" by Lumen Drift') and 'Paper Lanterns' in text and 'Whoosh' not in text, text)
    again = r.s.call('window.__nuzky.api.startExport(arguments[0], arguments[1], arguments[2], true)', str(video),
                     {'resolution': 720, 'fps': 30, 'quality': 'small'}, r.state()['epoch'])
    r.check('exporting over the video again asks before replacing its credits file',
            not again['ok'] and 'CREDITS_EXIST: reel.credits.txt already exists' in again['error'], again)
    duration, codecs = ffprobe(video)
    r.check('the video has the sound, as long as the timeline', 'aac' in codecs and abs(duration - 14.53) < 0.2, (duration, codecs))

    # Offline: built-in sounds still come up, and the panel says why online ones do not.
    r.openverse.stop()
    r.s.run(TYPE, 'Search sound effects', 'boom')
    offline = wait(lambda: r.s.run("return document.body.innerText.includes(\"You're offline\")"), 15)
    r.check("offline the panel says so and offers Try again", offline and r.s.run(
        "return [...document.querySelectorAll('button')].some((b) => b.textContent === 'Try again')"))
    r.check('the built-in Boom still shows', any(x['id'] == 'nuzky:boom' for x in rows(r)), rows(r))
    shot(r, 'effects-offline')

    if r.check('the window goes to its smallest size', resize(r, 1024, 640)):
        r.s.run(CLICK, '#audio-tab-music')
        time.sleep(0.5)
        shot(r, 'music-1024')
        r.s.run(CLICK, '#audio-tab-effects')
        time.sleep(0.5)
        shot(r, 'effects-1024')
    r.check('no error toasts', not r.errors(), r.errors())
