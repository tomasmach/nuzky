import time

from e2e.harness import flow, preview_crop, preview_rect


# Where the yellow marker of the stored top-left corner must appear on screen, per EXIF orientation.
ORIENTATIONS = {2: 'top-right', 4: 'bottom-left', 6: 'top-right', 7: 'bottom-right'}


def marker(screenshot, rect):
    """Corner of the yellow marker inside the photo, and whether the photo shows as portrait."""
    crop = preview_crop(screenshot, rect)
    yellow, photo = [], []
    pixels = crop.tobytes()
    for i in range(crop.width * crop.height):
        red, green, blue = pixels[3 * i:3 * i + 3]
        point = (i % crop.width, i // crop.width)
        if red > 180 and green > 140 and blue < 100:
            yellow.append(point)
            photo.append(point)
        elif blue > 140 and red < 110:
            photo.append(point)
    if not yellow or not photo:
        return None, None
    xs, ys = [p[0] for p in photo], [p[1] for p in photo]
    cx, cy = sum(p[0] for p in yellow) / len(yellow), sum(p[1] for p in yellow) / len(yellow)
    corner = ('top' if cy < (min(ys) + max(ys)) / 2 else 'bottom') + '-' + ('left' if cx < (min(xs) + max(xs)) / 2 else 'right')
    return corner, 'portrait' if max(ys) - min(ys) > max(xs) - min(xs) else 'landscape'


@flow('orientation', 'Phone photos with EXIF orientation 2, 4, 6 and 7 show the right way round in the preview')
def orientation(r):
    from PIL import Image
    photos = []
    for number in ORIENTATIONS:
        image = Image.new('RGB', (400, 200), (40, 90, 200))
        image.paste((240, 200, 30), (0, 0, 120, 60))
        exif = Image.Exif()
        exif[0x0112] = number
        photos.append(r.work / f'orientation-{number}.jpg')
        image.save(photos[-1], exif=exif.tobytes())
    r.import_media(*photos)
    for photo in photos:
        r.add_clip(photo.name)
    names = {a['id']: a['name'] for a in r.state()['assets']}
    rect = preview_rect(r)
    for clip in r.track():
        number = int(names[clip['assetId']].split('-')[1].split('.')[0])
        r.seek(clip['startUs'] + clip['durationUs'] // 2)
        time.sleep(1.5)
        r.shot(f'orientation-{number}')
        corner, shape = marker(r.work / f'orientation-{number}.png', rect)
        r.check(f'EXIF {number}: the marked corner shows {ORIENTATIONS[number]}', corner == ORIENTATIONS[number], corner)
        expected = 'portrait' if number >= 5 else 'landscape'
        r.check(f'EXIF {number}: the photo shows as {expected}', shape == expected, shape)
