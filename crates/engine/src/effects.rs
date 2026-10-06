use crate::model::{AnimationKind, Clip, ClipContent, Track, Transform, Transition};

const ZOOM_MAX_SCALE: f32 = 1.5;
const POP_MAX_SCALE: f32 = 1.1;

pub fn max_animation_scale(clip: &Clip) -> f32 {
    [(clip.anim_in, false), (clip.anim_out, true)].into_iter().map(|(animation, exiting)| {
        match animation.filter(|a| a.duration_us > 0).map(|a| a.kind) {
            Some(AnimationKind::ZoomIn) if exiting => ZOOM_MAX_SCALE,
            Some(AnimationKind::ZoomOut) if !exiting => ZOOM_MAX_SCALE,
            Some(AnimationKind::Pop) => POP_MAX_SCALE,
            _ => 1.0,
        }
    }).product()
}

pub fn source_time(clip: &Clip, t_us: i64) -> i64 {
    match clip.content {
        ClipContent::Media { source_in_us, speed, .. } => source_in_us + ((t_us - clip.start_us).max(0) as f64 * speed as f64).round() as i64,
        _ => 0,
    }
}

pub fn transition_window(clip: &Clip) -> Option<(i64, i64)> {
    let d = clip.transition_in?.duration_us;
    (d > 0).then_some((clip.start_us - d / 2, clip.start_us - d / 2 + d))
}

pub fn transition_at(track: &Track, t_us: i64) -> Option<(&Clip, &Clip, Transition, f32)> {
    if track.id != crate::edit::MAIN_TRACK {
        return None;
    }
    track.clips.windows(2).find_map(|pair| {
        let b = &pair[1];
        let (start, end) = transition_window(b)?;
        if t_us < start || t_us >= end {
            return None;
        }
        Some((&pair[0], b, b.transition_in?, (t_us - start) as f32 / (end - start) as f32))
    })
}

pub fn transform_at(clip: &Clip, t_us: i64) -> (Transform, f32) {
    let u = t_us - clip.start_us;
    let base = match clip.content {
        ClipContent::Media { transform, .. } | ClipContent::Text { transform, .. } => transform,
    };
    let mut transform = keyframed(clip, u).unwrap_or(base);
    let mut reveal = 1.0;
    for (animation, exiting) in [(clip.anim_in, false), (clip.anim_out, true)] {
        let Some(animation) = animation.filter(|a| a.duration_us > 0) else { continue };
        let duration = animation.duration_us.min(clip.duration_us).max(1);
        let elapsed = if exiting { u - (clip.duration_us - duration) } else { u };
        let progress = (elapsed as f32 / duration as f32).clamp(0.0, 1.0);
        let eased = if exiting { progress.powi(3) } else { 1.0 - (1.0 - progress).powi(3) };
        let visible = if exiting { 1.0 - eased } else { eased };
        if animation.kind == AnimationKind::Typewriter && matches!(clip.content, ClipContent::Text { .. }) {
            reveal *= visible;
        } else {
            animate(&mut transform, animation.kind, eased, exiting);
        }
    }
    (transform, reveal)
}

fn keyframed(clip: &Clip, u: i64) -> Option<Transform> {
    let first = clip.keyframes.first()?;
    if u <= first.t_us {
        return Some(first.transform);
    }
    for pair in clip.keyframes.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if u <= b.t_us {
            let p = (u - a.t_us) as f32 / (b.t_us - a.t_us).max(1) as f32;
            let lerp = |a: f32, b: f32| a + (b - a) * p;
            return Some(Transform {
                x: lerp(a.transform.x, b.transform.x), y: lerp(a.transform.y, b.transform.y),
                scale: lerp(a.transform.scale, b.transform.scale), rotation: lerp(a.transform.rotation, b.transform.rotation),
                opacity: lerp(a.transform.opacity, b.transform.opacity),
            });
        }
    }
    clip.keyframes.last().map(|k| k.transform)
}

fn animate(t: &mut Transform, kind: AnimationKind, p: f32, exiting: bool) {
    use AnimationKind::*;
    let visible = if exiting { 1.0 - p } else { p };
    t.opacity *= visible;
    let scale = match kind {
        ZoomIn => if exiting { 1.0 + (ZOOM_MAX_SCALE - 1.0) * p } else { 0.5 + 0.5 * p },
        ZoomOut => if exiting { 1.0 - 0.5 * p } else { ZOOM_MAX_SCALE - (ZOOM_MAX_SCALE - 1.0) * p },
        Pop => if p < 0.7 {
            let start = if exiting { 1.0 } else { 0.0 };
            start + (POP_MAX_SCALE - start) * p / 0.7
        } else {
            let end = if exiting { 0.0 } else { 1.0 };
            POP_MAX_SCALE + (end - POP_MAX_SCALE) * (p - 0.7) / 0.3
        },
        _ => 1.0,
    };
    t.scale *= scale;
    let distance = if exiting { -0.3 * p } else { 0.3 * (1.0 - p) };
    match kind {
        SlideUp => t.y += distance,
        SlideDown => t.y -= distance,
        SlideLeft => t.x += distance,
        SlideRight => t.x -= distance,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Animation, Keyframe, TransitionKind};

    fn clip() -> Clip {
        Clip::new("test".into(), 1_000_000, 4_000_000, ClipContent::Media {
            asset_id: "a".into(), source_in_us: 500_000, speed: 2.0, volume: 1.0,
            transform: Transform::default(), adjust: Default::default(), fade_in_us: 0, fade_out_us: 0,
        })
    }

    #[test]
    fn decode_scale_ignores_zero_length_animations() {
        let mut c = clip();
        c.anim_in = Some(Animation { kind: AnimationKind::ZoomOut, duration_us: 0 });
        c.anim_out = Some(Animation { kind: AnimationKind::Pop, duration_us: 0 });
        assert_eq!(max_animation_scale(&c), 1.0);
        c.anim_in.as_mut().unwrap().duration_us = 1_000_000;
        assert_eq!(max_animation_scale(&c), ZOOM_MAX_SCALE);
        c.anim_out.as_mut().unwrap().duration_us = 1_000_000;
        assert_eq!(max_animation_scale(&c), ZOOM_MAX_SCALE * POP_MAX_SCALE);
    }

    #[test]
    fn source_speed_and_keyframe_holds() {
        let mut c = clip();
        assert_eq!(source_time(&c, 2_000_000), 2_500_000);
        assert_eq!(source_time(&c, 0), 500_000);
        let a = Transform { x: -0.5, y: 0.2, scale: 0.5, rotation: -90.0, opacity: 0.2 };
        let b = Transform { x: 0.5, y: 0.6, scale: 1.5, rotation: 90.0, opacity: 0.8 };
        c.keyframes = vec![Keyframe { t_us: 0, transform: a }, Keyframe { t_us: 2_000_000, transform: b }];
        assert_eq!(transform_at(&c, 0).0, a);
        assert_eq!(transform_at(&c, 5_000_000).0, b);
        let mid = transform_at(&c, 2_000_000).0;
        assert_eq!((mid.x, mid.scale, mid.rotation, mid.opacity), (0.0, 1.0, 0.0, 0.5));
        assert!((mid.y - 0.4).abs() < 1e-6);
    }

    #[test]
    fn animation_curves_and_directions() {
        use AnimationKind::*;
        for kind in [Fade, ZoomIn, ZoomOut, SlideUp, SlideDown, SlideLeft, SlideRight, Pop, Typewriter] {
            for exiting in [false, true] {
                let mut c = clip();
                let a = Some(Animation { kind, duration_us: 1_000_000 });
                if exiting { c.anim_out = a; } else { c.anim_in = a; }
                let start = if exiting { c.end_us() - 1_000_000 } else { c.start_us };
                for (offset, expected) in [(0, if exiting { 1.0 } else { 0.0 }), (500_000, 0.875), (1_000_000, if exiting { 0.0 } else { 1.0 })] {
                    assert!((transform_at(&c, start + offset).0.opacity - expected).abs() < 1e-6);
                }
                let mid = transform_at(&c, start + 500_000).0;
                if kind == SlideUp { assert!((mid.y - if exiting { -0.0375 } else { 0.0375 }).abs() < 1e-6); }
                if kind == ZoomIn { assert!((mid.scale - if exiting { 1.0625 } else { 0.9375 }).abs() < 1e-6); }
                if kind == ZoomOut { assert!((mid.scale - if exiting { 0.9375 } else { 1.0625 }).abs() < 1e-6); }
            }
        }
        let mut t = Transform::default();
        animate(&mut t, Pop, 0.7, false);
        assert!((t.scale - 1.1).abs() < 1e-6);
    }

    #[test]
    fn centred_transition_progress() {
        let mut b = clip();
        b.transition_in = Some(Transition { kind: TransitionKind::Dissolve, duration_us: 1_000_000 });
        let track = Track { id: "main".into(), kind: crate::model::TrackKind::Video, name: String::new(), muted: false, hidden: false, keep_in_place: false, clips: vec![clip(), b] };
        assert!(transition_at(&track, 499_999).is_none());
        for (t, p) in [(500_000, 0.0), (1_000_000, 0.5), (1_499_999, 0.999999)] {
            assert!((transition_at(&track, t).unwrap().3 - p).abs() < 1e-6);
        }
        assert!(transition_at(&track, 1_500_000).is_none());
    }
}
