#!/usr/bin/env python3
"""Builds the sound effects built into Nuzky, assets/sounds, from their CC0 sources.

Each source is downloaded once, checked against its pinned SHA-256 and kept in ~/.cache/nuzky/deps/sounds.
Every sound then loses the silence at its edges, is shortened where the list says so, is levelled to a
sample peak of -2 dBFS and is encoded as Ogg Opus. manifest.json records the author, source page and
licence of each, and what was changed. The output is committed; run this only after changing the list.

    python3 -I scripts/build-sounds.py
"""
import hashlib, json, os, re, subprocess, sys, tempfile, urllib.request, zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'assets' / 'sounds'
CACHE = Path.home() / '.cache' / 'nuzky' / 'deps' / 'sounds'
CC0 = 'https://creativecommons.org/publicdomain/zero/1.0/'
PEAK_DB = -2.0
AGENT = 'Nuzky-build/1 (https://nuzky.app)'

# Every source states CC0 on its page; the Kenney archives carry it in License.txt too.
SOURCES = {
    'KI': ('Kenney', 'https://kenney.nl/assets/interface-sounds',
           'https://kenney.nl/media/pages/assets/interface-sounds/fa43c1dd4d-1677589452/kenney_interface-sounds.zip',
           'f2193d072726d6758a5f7871b2dcc54dcce0d5c35c6f0a62f92549b327c81232'),
    'KM': ('Kenney', 'https://kenney.nl/assets/impact-sounds',
           'https://kenney.nl/media/pages/assets/impact-sounds/87b4ddecda-1677589768/kenney_impact-sounds.zip',
           '029d734af1582474edf3a694d1b0cebc97c1c152f2f39fa34d4c2bafc5de77f8'),
    'KD': ('Kenney', 'https://kenney.nl/assets/digital-audio',
           'https://kenney.nl/media/pages/assets/digital-audio/216eac4753-1677590265/kenney_digital-audio.zip',
           '24e6ce28b76a6d8c89cff4d331e0965ff5c3de8a73c612028e9d363cc64e4f06'),
    'KS': ('Kenney', 'https://kenney.nl/assets/sci-fi-sounds',
           'https://kenney.nl/media/pages/assets/sci-fi-sounds/6b296f9ecf-1677589334/kenney_sci-fi-sounds.zip',
           '119340f351a5098ad814f78719438c0da355a9ce8a4c8a3af6a8d48aa3d49e04'),
    'SW': ('artisticdude', 'https://opengameart.org/content/swishes-sound-pack',
           'https://opengameart.org/sites/default/files/swishes.zip',
           '7980215241b739a787dcf26f660ce510bb237900968789395a86911619795693'),
    'TY': ('unicaegames', 'https://opengameart.org/content/keyboard-soundpack-1-typing-and-single-keystrokes',
           'https://opengameart.org/sites/default/files/unicae_games_keyboard_soundpack_1_0.zip',
           '935eae2fa5c3742eacdd38c4ea0e9047f3887faa0701996498492c256db1b351'),
    'PO': ('farfadet46', 'https://opengameart.org/content/bubbles-pop',
           'https://opengameart.org/sites/default/files/pop.ogg',
           'd55886ec5b145045d1abd13926675b93646a8865e53e1f7a3a58cfbb7b733cff'),
    'BO': ('Aeva', 'https://opengameart.org/content/boing',
           'https://opengameart.org/sites/default/files/boing.flac',
           '9c6af38ca79332ad3fa66ad1229179d2a91655c1157d3620e21ef676f54d9fdd'),
    'AP': ('eXpl0it3r', 'https://opengameart.org/content/applause-in-a-large-hall-or-church',
           'https://opengameart.org/sites/default/files/applause-clapping-church-crowd-immersive.wav',
           '0d3bfde5a050f3c5e6685bd7f1efc7ab1d289fbad65c88c1ce9b4691228c69c9'),
    'CO': ('Spring Spring', 'https://opengameart.org/content/purchasing-sound-effect',
           'https://opengameart.org/sites/default/files/snd_purchase_0.wav',
           '091bfc1a3ee7ebd7713636ec631c940b79de715e141856e6c0daf09e8868486e'),
    'PH': ('themightyglider, from a recording by A Clock in the Kingdom', 'https://opengameart.org/content/camera',
           'https://opengameart.org/sites/default/files/photo.ogg',
           '029dfcb981fdf375fb0ae4657962b3d470e1a073932c3d3aefb15d3d9ba38670'),
    'CA': ('CapsLok', 'https://freesound.org/people/CapsLok/sounds/184438/',
           'https://cdn.freesound.org/previews/184/184438_850742-hq.mp3', 'efd380e06aa7abbf2e779462281f37ad3b862c44309145d60e9dcca76c1637b2'),
    'RS': ('florianreichelt', 'https://freesound.org/people/florianreichelt/sounds/683322/',
           'https://cdn.freesound.org/previews/683/683322_6253486-hq.mp3', 'f07324361b017c5643564682d3480a9b8a5024faba0053e34626cca10745cbe7'),
    'SP': ('MLaudio', 'https://freesound.org/people/MLaudio/sounds/511485/',
           'https://cdn.freesound.org/previews/511/511485_6890478-hq.mp3', '391d56061a84d6cca6be41c8a3bd5cc5780540fa6302016778d5873979411ad6'),
    'DR': ('gnuoctathorpe', 'https://freesound.org/people/gnuoctathorpe/sounds/404857/',
           'https://cdn.freesound.org/previews/404/404857_413392-hq.mp3', 'ecdb47d9cee5816b0a7c73a35046e86955dcf1798194cabdf9683ea1183f3d6c'),
    'W3': ('Joseph Sardin', 'https://bigsoundbank.com/whoosh-3-s1795.html',
           'https://bigsoundbank.com/UPLOAD/mp3/1795.mp3',
           '0fb0a12bc292b6a56ed03b37d9c18e2686d7eba1026fc6f4c2c58d1549078a03'),
    'W4': ('Joseph Sardin', 'https://bigsoundbank.com/whoosh-4-s1796.html',
           'https://bigsoundbank.com/UPLOAD/mp3/1796.mp3',
           '046e8bb86385a42b60813a56d1a7404e0b78b84c0689c9a5c14e50e7bb087f8b'),
    'LH': ('Joseph Sardin', 'https://bigsoundbank.com/laughter-s0490.html',
           'https://bigsoundbank.com/UPLOAD/mp3/0490.mp3',
           '4c58542c47ecce9b3d994eaec33b7c70f8551f2247e8de370829bda455a06933'),
    'HB': ('Joseph Sardin', 'https://bigsoundbank.com/heartbeat-3-s0770.html',
           'https://bigsoundbank.com/UPLOAD/mp3/0770.mp3',
           '6004a6d4001f30d6c1185fea31900d1fe16c5e4d3478d0d4b184260fc982dd44'),
}

# id, title, category, source, file inside the archive, seconds kept (None keeps all of it).
SOUNDS = [
    ('whoosh', 'Whoosh', 'Whoosh', 'W3', None, None),
    ('swoosh', 'Swoosh', 'Whoosh', 'W4', None, None),
    ('swish-short', 'Swish, short', 'Whoosh', 'SW', 'swishes/swish-9.wav', None),
    ('swish-tiny', 'Swish, tiny', 'Whoosh', 'SW', 'swishes/swish-1.wav', None),
    ('swish-tiny-2', 'Swish, tiny 2', 'Whoosh', 'SW', 'swishes/swish-5.wav', None),
    ('riser-retro', 'Retro riser', 'Transition', 'KD', 'Audio/highUp.ogg', None),
    ('phaser-sweep', 'Phaser sweep', 'Transition', 'KD', 'Audio/phaserUp1.ogg', None),
    ('swipe-in', 'Swipe in', 'Transition', 'KI', 'Audio/maximize_001.ogg', None),
    ('swipe-out', 'Swipe out', 'Transition', 'KI', 'Audio/minimize_001.ogg', None),
    ('glitch', 'Glitch', 'Transition', 'KI', 'Audio/glitch_001.ogg', None),
    ('glitch-2', 'Glitch 2', 'Transition', 'KI', 'Audio/glitch_002.ogg', None),
    ('bubble-pop', 'Bubble pop', 'Pop and click', 'PO', None, None),
    ('click', 'Click', 'Pop and click', 'KI', 'Audio/click_001.ogg', None),
    ('click-sharp', 'Click, sharp', 'Pop and click', 'KI', 'Audio/click_002.ogg', None),
    ('tap', 'Tap', 'Pop and click', 'KI', 'Audio/tick_002.ogg', None),
    ('wood-tap', 'Wood tap', 'Pop and click', 'KM', 'Audio/impactWood_light_000.ogg', None),
    ('key-press', 'Key press', 'Pop and click', 'TY', 'Single Keys/keypress-001.wav', None),
    ('ding', 'Ding', 'Bell and alert', 'KI', 'Audio/glass_001.ogg', None),
    ('bell', 'Bell', 'Bell and alert', 'KM', 'Audio/impactBell_heavy_000.ogg', None),
    ('bell-long', 'Bell, long', 'Bell and alert', 'KM', 'Audio/impactBell_heavy_001.ogg', None),
    ('notification', 'Notification', 'Bell and alert', 'KD', 'Audio/threeTone1.ogg', None),
    ('success', 'Success', 'Bell and alert', 'KI', 'Audio/confirmation_001.ogg', None),
    ('success-chime', 'Success chime', 'Bell and alert', 'KI', 'Audio/confirmation_002.ogg', None),
    ('error', 'Error', 'Bell and alert', 'KI', 'Audio/error_001.ogg', None),
    ('boom', 'Boom', 'Impact', 'KS', 'Audio/lowFrequency_explosion_000.ogg', None),
    ('punch', 'Punch', 'Impact', 'KM', 'Audio/impactPunch_heavy_000.ogg', None),
    ('soft-hit', 'Soft hit', 'Impact', 'KM', 'Audio/impactSoft_heavy_000.ogg', None),
    ('metal-hit', 'Metal hit', 'Impact', 'KM', 'Audio/impactMetal_heavy_000.ogg', None),
    ('crunch', 'Crunch', 'Impact', 'KS', 'Audio/explosionCrunch_000.ogg', None),
    ('rimshot', 'Rimshot', 'Impact', 'DR', None, None),
    ('boing', 'Boing', 'Funny', 'BO', None, None),
    ('slime', 'Slime', 'Funny', 'KS', 'Audio/slime_000.ogg', None),
    ('power-up', 'Power-up', 'Funny', 'KD', 'Audio/powerUp1.ogg', None),
    ('record-scratch', 'Record scratch', 'Funny', 'RS', None, None),
    ('sparkle', 'Sparkle', 'Funny', 'SP', None, None),
    ('laugh', 'Laugh', 'Funny', 'LH', None, None),
    ('applause', 'Applause', 'Funny', 'AP', None, 8),
    ('camera-shutter', 'Camera shutter', 'Everyday', 'PH', None, None),
    ('cash-register', 'Cash register', 'Everyday', 'CA', None, None),
    ('coins', 'Coins', 'Everyday', 'CO', None, None),
    ('typing', 'Typing', 'Everyday', 'TY', 'Human Typing/human_vel-002.wav', 5),
    ('typing-steady', 'Typing, steady', 'Everyday', 'TY', 'Generated Typing/generated-004_medium.wav', None),
    ('heartbeat', 'Heartbeat', 'Everyday', 'HB', None, 8),
]

# After levelling, silence more than 60 dB under the peak goes from the start and the end, never from
# inside; a few milliseconds stay before the sound and 50 ms after it.
EDGES = ('silenceremove=start_periods=1:start_threshold=-60dB:detection=peak:start_silence=0.005,areverse,'
         'silenceremove=start_periods=1:start_threshold=-60dB:detection=peak:start_silence=0.05,areverse')
FADE = 1.0


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def fetch(key):
    _, _, url, pinned = SOURCES[key]
    if (CACHE / pinned).is_file():
        return CACHE / pinned
    CACHE.mkdir(parents=True, exist_ok=True)
    request = urllib.request.Request(url, headers={'User-Agent': AGENT})
    with urllib.request.urlopen(request, timeout=60) as response:
        data = response.read()
    got = hashlib.sha256(data).hexdigest()
    if got != pinned:
        sys.exit(f'{url}: SHA-256 {got}, expected {pinned}')
    (CACHE / got).write_bytes(data)
    return CACHE / got


def source_file(key, member, scratch):
    path = fetch(key)
    if member is None:
        return path
    target = Path(scratch) / key / member
    if not target.is_file():
        with zipfile.ZipFile(path) as archive:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(archive.read(member))
    return target


def probe(path, entry):
    out = subprocess.run(['ffprobe', '-v', 'error', '-select_streams', 'a:0', '-show_entries', entry,
                          '-of', 'default=nw=1:nk=1', str(path)], capture_output=True, text=True, check=True)
    return out.stdout.strip()


def build(sound, scratch):
    id, title, category, key, member, keep = sound
    src = source_file(key, member, scratch)
    measured = subprocess.run(['ffmpeg', '-hide_banner', '-nostats', '-i', str(src), '-af',
                               (f'atrim=0:{keep},' if keep else '') + 'volumedetect', '-f', 'null', '-'],
                              capture_output=True, text=True, check=True).stderr
    peak = float(re.search(r'max_volume: (-?[\d.]+) dB', measured).group(1))
    chain = f'volume={PEAK_DB - peak:.2f}dB,{EDGES}'
    if keep:
        chain += f',atrim=0:{keep},afade=t=out:st={keep - FADE}:d={FADE}'
    channels = min(int(probe(src, 'stream=channels')), 2)
    out = OUT / f'{id}.ogg'
    subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-i', str(src), '-af', chain,
                    '-ac', str(channels), '-ar', '48000', '-c:a', 'libopus', '-b:a', '64k' if channels == 1 else '96k',
                    '-map_metadata', '-1', '-fflags', '+bitexact', '-flags:a', '+bitexact', str(out)], check=True)
    author, page, url, pinned = SOURCES[key]
    changes = 'Peak level -2 dBFS, silence trimmed at the start and end, encoded as Ogg Opus'
    if keep:
        changes += f'; the first {keep} s, fading out over the last second'
    return {
        'id': id, 'file': out.name, 'title': title, 'category': category, 'author': author, 'source': page,
        'license': 'CC0 1.0', 'licenseUrl': CC0, 'download': url, 'downloadSha256': pinned, 'member': member,
        'changes': changes, 'durationUs': round(float(probe(out, 'stream=duration')) * 1e6), 'sha256': sha256(out),
    }


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as scratch:
        manifest = [build(sound, scratch) for sound in SOUNDS]
    for stale in set(OUT.glob('*.ogg')) - {OUT / s['file'] for s in manifest}:
        stale.unlink()
    (OUT / 'manifest.json').write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + '\n')
    total = sum((OUT / s['file']).stat().st_size for s in manifest)
    print(f'{len(manifest)} sounds, {total / 1e6:.2f} MB in {OUT.relative_to(ROOT)}')


if __name__ == '__main__':
    main()
