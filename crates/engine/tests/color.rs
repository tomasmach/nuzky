use nuzky_engine::model::{Adjust, ClipContent};
use nuzky_engine::{Project, Renderer, Wait};

fn ramp(renderer: &mut Renderer, adjust: Adjust) -> Vec<u8> {
    let project: Project = serde_json::from_value(serde_json::json!({
        "version": 1, "name": "Color ramp",
        "canvas": { "width": 256, "height": 16, "fps": 30, "background": "#000000" },
        "assets": [{
            "id": "ramp", "name": "Ramp",
            "path": concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/color-ramp.ppm"),
            "kind": "image", "durationUs": 0, "width": 256, "height": 16,
            "fps": 0, "hasAudio": false
        }],
        "tracks": [{ "id": "main", "kind": "video", "clips": [{
            "id": "ramp", "startUs": 0, "durationUs": 1_000_000,
            "content": {
                "type": "media", "assetId": "ramp", "sourceInUs": 0,
                "speed": 1, "volume": 1,
                "transform": { "x": 0, "y": 0, "scale": 1, "rotation": 0, "opacity": 1 },
                "adjust": adjust
            }
        }] }]
    }))
    .unwrap();
    // Exercise deserialization and the same image decode/composition path as CLI frames.
    let ClipContent::Media { adjust: loaded, .. } = &project.tracks[0].clips[0].content else { panic!() };
    assert_eq!(*loaded, adjust);
    renderer.render(&project, 0, 256, 16, Wait::Exact, false).unwrap()
}

fn grey(frame: &[u8], x: usize) -> i32 {
    frame[(4 * 256 + x) * 4] as i32
}

fn smooth_grey(frame: &[u8]) {
    for x in 0..255 {
        let step = grey(frame, x + 1) - grey(frame, x);
        assert!((0..=3).contains(&step), "non-monotonic or abrupt ramp at {x}: {step}");
    }
    assert!(frame.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
}

// Golden RGBA captured with the unmodified a18d535 CLI: frame ramp.json 0 ramp-head.png 256.
// 256x16 image layer, default transform/Adjust, RADV NAVI21; upper half grey, lower half RGB.
#[test]
fn zero_adjust_matches_head_a18d535_frame() {
    let mut renderer = Renderer::new().unwrap();
    let frame = ramp(&mut renderer, Adjust::default());
    assert_eq!(frame.as_slice(), include_bytes!("fixtures/color-ramp-head-a18d535.rgba"));
    let old: Adjust =
        serde_json::from_str(r#"{"brightness":0,"contrast":0,"saturation":0,"temperature":0,"vignette":0}"#).unwrap();
    assert_eq!(old, Adjust::default());
    assert_eq!(ramp(&mut renderer, old), frame);
}

#[test]
fn exposure_uses_linear_stops_and_preserves_black() {
    let mut renderer = Renderer::new().unwrap();
    let base = ramp(&mut renderer, Adjust::default());
    for amount in [-1.0, 1.0] {
        let frame = ramp(&mut renderer, Adjust { exposure: amount, ..Adjust::default() });
        let mean = |f: &[u8]| (0..256).map(|x| grey(f, x)).sum::<i32>();
        assert_eq!(grey(&frame, 0), 0);
        assert_eq!(mean(&frame) > mean(&base), amount > 0.0);
        // sRGB 0.25 -> linear ~0.051; +/-2 stops -> sRGB ~0.489 / ~0.116.
        let expected = if amount > 0.0 { 125 } else { 30 };
        assert!((grey(&frame, 64) - expected).abs() <= 2);
    }
}

#[test]
fn tint_moves_green_against_red_and_blue() {
    let mut renderer = Renderer::new().unwrap();
    for amount in [-1.0, 1.0] {
        let frame = ramp(&mut renderer, Adjust { tint: amount, ..Adjust::default() });
        let p = &frame[(4 * 256 + 128) * 4..][..4];
        assert_eq!(p[0] > p[1] && p[2] > p[1], amount > 0.0);
        assert_eq!(p[0] < p[1] && p[2] < p[1], amount < 0.0);
    }
}

#[test]
fn highlights_target_brights_and_preserve_midgrey() {
    let mut renderer = Renderer::new().unwrap();
    let base = ramp(&mut renderer, Adjust::default());
    for amount in [-1.0, 1.0] {
        let frame = ramp(&mut renderer, Adjust { highlights: amount, ..Adjust::default() });
        let delta = grey(&frame, 230) - grey(&base, 230);
        assert_eq!(delta > 0, amount > 0.0);
        assert!(delta.abs() > 4 * (grey(&frame, 128) - grey(&base, 128)).abs() + 4);
        assert!((grey(&frame, 128) - grey(&base, 128)).abs() <= 1);
        assert_eq!(grey(&frame, 32), grey(&base, 32));
        smooth_grey(&frame);
    }
}

#[test]
fn shadows_target_darks_and_preserve_midgrey() {
    let mut renderer = Renderer::new().unwrap();
    let base = ramp(&mut renderer, Adjust::default());
    for amount in [-1.0, 1.0] {
        let frame = ramp(&mut renderer, Adjust { shadows: amount, ..Adjust::default() });
        let delta = grey(&frame, 25) - grey(&base, 25);
        assert_eq!(delta > 0, amount > 0.0);
        assert!(delta.abs() > 4 * (grey(&frame, 128) - grey(&base, 128)).abs() + 4);
        assert!((grey(&frame, 128) - grey(&base, 128)).abs() <= 1);
        assert_eq!(grey(&frame, 230), grey(&base, 230));
        smooth_grey(&frame);
    }
}

#[test]
fn fade_lifts_black_and_gently_compresses_white() {
    let mut renderer = Renderer::new().unwrap();
    let base = ramp(&mut renderer, Adjust::default());
    for amount in [0.5, 1.0] {
        let frame = ramp(&mut renderer, Adjust { fade: amount, ..Adjust::default() });
        assert!(grey(&frame, 0) > grey(&base, 0));
        assert!((grey(&frame, 0) as f32 - 255.0 * 0.25 * amount).abs() <= 1.0);
        assert!(grey(&frame, 255) < 255 && grey(&frame, 255) >= 242);
        smooth_grey(&frame);
    }
}
