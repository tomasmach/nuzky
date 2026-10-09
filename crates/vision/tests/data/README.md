# Vision test data

`face-open-person.json` outlines the person in `tmp-test/face-open.png`, which `scripts/fixtures.sh` builds.
The polygon is in pixels of that 1080x1920 still and covers the head with headband, hair and bun, ears,
neck, collar and the flight suit down to the bottom edge. The shoulders run out of both sides of the frame.
It was traced by hand on 2x to 4x zooms and checked by rasterising it over the image. It follows the
silhouette within a few pixels. The thin loose hair strands under the bun (about x 850 to 905, y 750 to 830)
are left outside because they are mostly background.

## Source

The stills and `tmp-test/face-thumb.mp4` come from "Artemis Interviews with NASA Astronaut Jeanette Epps -
October 4, 2019", https://images.nasa.gov/details/iss061m2627771232_Live_Interviews_Jeanette_Epps_191004.
The script downloads only the first 10 MiB of
`https://images-assets.nasa.gov/video/iss061m2627771232_Live_Interviews_Jeanette_Epps_191004/iss061m2627771232_Live_Interviews_Jeanette_Epps_191004~large.mp4`
(1280x720, 59.94 fps), pinned by SHA-256. `face-open.png` is the frame at 8.992 s and `face-blink.png` the
frame at 9.159 s, both cropped to 396x704 at (447, 8) and scaled to 1080x1920.

## Licence

NASA video is a work of the US government and generally not subject to copyright in the United States, see
https://www.nasa.gov/nasa-brand-center/images-and-media/. The same page says the NASA insignia on the suit
and the Artemis logo in the background are not in the public domain, and that the likeness of an
identifiable astronaut must not be used commercially or to imply endorsement. The media therefore stay out
of git: the script downloads them for local tests only.
