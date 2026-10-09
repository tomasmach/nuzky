//! ONNX Runtime sessions of the models, each with its own input layout and output decoding.
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use ort::session::{RunOptions, Session, builder::GraphOptimizationLevel};
use ort::value::TensorRef;

use crate::frame::{Frame, Rect};

fn ort_error<E: std::fmt::Display>(error: E) -> anyhow::Error {
    anyhow!("ONNX Runtime: {error}")
}

/// `threads` 0 lets ONNX Runtime use every core; the small models run one thread each, beside
/// other workers.
pub(crate) fn session(path: &Path, threads: usize) -> Result<Session> {
    crate::runtime::require()?;
    let mut builder = Session::builder()
        .map_err(ort_error)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort_error)?;
    if threads > 0 {
        builder = builder.with_intra_threads(threads).map_err(ort_error)?;
    }
    builder.commit_from_file(path).map_err(ort_error).with_context(|| format!("Loading {}", path.display()))
}

pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("CANCELLED: job cancelled");
    }
    Ok(())
}

/// One float output, owned.
pub(crate) struct Output {
    pub shape: Vec<i64>,
    pub data: Vec<f32>,
}

/// Runs a model on one float tensor and copies its float outputs, in the model's output order.
/// A watcher stops the run within a few tens of milliseconds of `cancel`, even mid-inference.
pub(crate) fn run(session: &mut Session, shape: &[usize], input: &[f32], cancel: &AtomicBool) -> Result<Vec<Output>> {
    check_cancel(cancel)?;
    let options = RunOptions::new().map_err(ort_error)?;
    let done = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                if cancel.load(Ordering::Relaxed) {
                    let _ = options.terminate();
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(20));
            }
        });
        let result = (|| {
            let tensor = TensorRef::from_array_view((shape.to_vec(), input)).map_err(ort_error)?;
            let outputs = session.run_with_options(ort::inputs![tensor], &options).map_err(ort_error)?;
            outputs
                .iter()
                .map(|(_, value)| {
                    let (shape, data) = value.try_extract_tensor::<f32>().map_err(ort_error)?;
                    Ok(Output { shape: shape.to_vec(), data: data.to_vec() })
                })
                .collect::<Result<Vec<_>>>()
        })();
        done.store(true, Ordering::Release);
        watcher.thread().unpark();
        result
    });
    // A terminated run fails; report it as the cancellation it was.
    check_cancel(cancel)?;
    result
}

/// One face found by YuNet, in the pixels of the frame it looked at.
#[derive(Clone, Debug)]
pub struct Detection {
    pub rect: Rect,
    pub score: f32,
    /// Right eye, left eye, nose tip, right and left mouth corner (the person's sides).
    pub points: [[f32; 2]; 5],
}

impl Detection {
    pub fn scaled(&self, k: f32) -> Detection {
        Detection { rect: self.rect.map(|v| v * k), score: self.score, points: self.points.map(|p| p.map(|v| v * k)) }
    }
}

/// YuNet (OpenCV Zoo): BGR 0..255 in, padded to a multiple of 32; decoding follows OpenCV's
/// `FaceDetectorYN`.
pub struct Yunet(Session);

const YUNET_STRIDES: [usize; 3] = [8, 16, 32];
const YUNET_MIN_SCORE: f32 = 0.6;
const YUNET_NMS: f32 = 0.3;

impl Yunet {
    pub fn load(dir: &Path) -> Result<Self> {
        Ok(Self(session(&crate::models::YUNET.path(dir), 1)?))
    }

    pub fn detect(&mut self, frame: &Frame, cancel: &AtomicBool) -> Result<Vec<Detection>> {
        let (w, h) = (frame.width as usize, frame.height as usize);
        let (pw, ph) = (w.div_ceil(32) * 32, h.div_ceil(32) * 32);
        let mut input = vec![0f32; 3 * pw * ph];
        for y in 0..h {
            for x in 0..w {
                let p = &frame.rgba[(y * w + x) * 4..][..3];
                for (plane, c) in [2, 1, 0].into_iter().enumerate() {
                    input[plane * pw * ph + y * pw + x] = p[c] as f32;
                }
            }
        }
        let outputs = run(&mut self.0, &[1, 3, ph, pw], &input, cancel)?;
        if outputs.len() != 12 {
            bail!("Unexpected face detector outputs");
        }
        let mut found = Vec::new();
        for (i, stride) in YUNET_STRIDES.into_iter().enumerate() {
            let (cls, obj, bbox, kps) =
                (&outputs[i].data, &outputs[i + 3].data, &outputs[i + 6].data, &outputs[i + 9].data);
            let cols = pw / stride;
            for idx in 0..cls.len().min(obj.len()) {
                let score = (cls[idx].clamp(0.0, 1.0) * obj[idx].clamp(0.0, 1.0)).sqrt();
                if score < YUNET_MIN_SCORE {
                    continue;
                }
                let (c, r) = ((idx % cols) as f32, (idx / cols) as f32);
                let s = stride as f32;
                let b = &bbox[idx * 4..][..4];
                let (cx, cy) = ((c + b[0]) * s, (r + b[1]) * s);
                let (bw, bh) = (b[2].exp() * s, b[3].exp() * s);
                let k = &kps[idx * 10..][..10];
                let points = std::array::from_fn(|n| [(k[2 * n] + c) * s, (k[2 * n + 1] + r) * s]);
                found.push(Detection { rect: [cx - bw / 2.0, cy - bh / 2.0, bw, bh], score, points });
            }
        }
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut kept: Vec<Detection> = Vec::new();
        for face in found {
            if kept.iter().all(|k| iou(k.rect, face.rect) <= YUNET_NMS) {
                kept.push(face);
            }
        }
        Ok(kept)
    }
}

pub fn iou(a: Rect, b: Rect) -> f32 {
    let w = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let h = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    let both = w.max(0.0) * h.max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - both;
    if union <= 0.0 { 0.0 } else { both / union }
}

/// BiRefNet_lite: the whole frame squeezed to 1024², ImageNet normalisation, one logit per pixel.
pub struct BiRefNet(Session);

pub const BIREFNET_SIDE: usize = 1024;

impl BiRefNet {
    pub fn load(dir: &Path) -> Result<Self> {
        Ok(Self(session(&crate::models::BIREFNET.path(dir), 0)?))
    }

    /// Foreground probability per pixel of a 1024×1024 grid over the frame.
    pub fn segment(&mut self, frame: &Frame, cancel: &AtomicBool) -> Result<Vec<f32>> {
        const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
        const STD: [f32; 3] = [0.229, 0.224, 0.225];
        let n = BIREFNET_SIDE;
        let small = frame.resized(n as u32, n as u32);
        let mut input = vec![0f32; 3 * n * n];
        for (i, p) in small.rgba.chunks_exact(4).enumerate() {
            for c in 0..3 {
                input[c * n * n + i] = (p[c] as f32 / 255.0 - MEAN[c]) / STD[c];
            }
        }
        let outputs = run(&mut self.0, &[1, 3, n, n], &input, cancel)?;
        let logits = outputs.into_iter().next().context("Subject mask model gave no output")?;
        if logits.data.len() != n * n {
            bail!("Unexpected subject mask size {:?}", logits.shape);
        }
        Ok(logits.data.into_iter().map(|v| 1.0 / (1.0 + (-v).exp())).collect())
    }
}

/// 478 face landmarks in the pixels of the frame, and the square crop the model saw.
pub struct Landmarks {
    pub points: Vec<[f32; 3]>,
    pub crop: Frame,
    /// Angle of the eye line in the frame, radians.
    pub roll: f32,
}

/// MediaPipe Face Mesh V2 on a square crop around the face, turned so the eyes are level and
/// 1.5 times the face box, as MediaPipe's face landmarker crops.
pub struct FaceMesh(Session);

const MESH_SIDE: usize = 256;
const MESH_SCALE: f32 = 1.5;
const MESH_MIN_PRESENCE: f32 = 0.5;

impl FaceMesh {
    pub fn load(dir: &Path) -> Result<Self> {
        Ok(Self(session(&crate::models::FACE_MESH.path(dir), 1)?))
    }

    /// None when the model does not see a face in the crop after all.
    pub fn landmarks(&mut self, frame: &Frame, face: &Detection, cancel: &AtomicBool) -> Result<Option<Landmarks>> {
        let [right_eye, left_eye] = [face.points[0], face.points[1]];
        let roll = (left_eye[1] - right_eye[1]).atan2(left_eye[0] - right_eye[0]);
        let (cx, cy) = (face.rect[0] + face.rect[2] / 2.0, face.rect[1] + face.rect[3] / 2.0);
        let side = face.rect[2].max(face.rect[3]) * MESH_SCALE;
        let (cos, sin) = (roll.cos(), roll.sin());
        let n = MESH_SIDE as f32;
        let to_frame = |u: f32, v: f32| {
            let (dx, dy) = ((u / n - 0.5) * side, (v / n - 0.5) * side);
            [cx + cos * dx - sin * dy, cy + sin * dx + cos * dy]
        };
        let mut input = Vec::with_capacity(MESH_SIDE * MESH_SIDE * 3);
        let mut rgba = Vec::with_capacity(MESH_SIDE * MESH_SIDE * 4);
        for v in 0..MESH_SIDE {
            for u in 0..MESH_SIDE {
                let [x, y] = to_frame(u as f32 + 0.5, v as f32 + 0.5);
                let p = frame.sample(x, y);
                input.extend(p);
                rgba.extend(p.map(|c| (c * 255.0).round() as u8));
                rgba.push(255);
            }
        }
        let outputs = run(&mut self.0, &[1, MESH_SIDE, MESH_SIDE, 3], &input, cancel)?;
        // Outputs: landmarks (x, y, z in crop pixels), presence logit, tongue out.
        let [points, presence, ..] = &outputs[..] else { bail!("Unexpected face landmark outputs") };
        anyhow::ensure!(points.data.len() == 478 * 3, "Unexpected face landmark count {}", points.data.len());
        if 1.0 / (1.0 + (-presence.data[0]).exp()) < MESH_MIN_PRESENCE {
            return Ok(None);
        }
        let points = points
            .data
            .chunks_exact(3)
            .map(|p| {
                let [x, y] = to_frame(p[0], p[1]);
                [x, y, p[2] * side / n]
            })
            .collect();
        let crop = Frame { width: MESH_SIDE as u32, height: MESH_SIDE as u32, rgba };
        Ok(Some(Landmarks { points, crop, roll }))
    }
}

/// What a cover cares about in a face, from MediaPipe's blendshapes, each 0..1 except `yaw`.
#[derive(Clone, Debug)]
pub struct Expression {
    /// The more closed eye.
    pub blink: f32,
    /// Mouth corners up. Some faces read as smiling at rest, so compare it with the same face.
    pub smile: f32,
    /// Gap between the inner lips over the mouth's inner width; about 0 when closed.
    pub lips_apart: f32,
    /// Lips pushed forward, as for an o or a u.
    pub purse: f32,
    /// Brows down, nose wrinkled, upper lip raised or mouth corners down.
    pub frown: f32,
    /// The eyes' strongest look away from straight ahead.
    pub look_away: f32,
    /// Head turn: the nose tip's offset from between the eyes, in eye widths; 0 faces the camera.
    pub yaw: f32,
}

/// MediaPipe Blendshape V2: 146 of the landmarks as (x, y) in image pixels, 52 scores out.
pub struct Blendshapes(Session);

impl Blendshapes {
    pub fn load(dir: &Path) -> Result<Self> {
        Ok(Self(session(&crate::models::BLENDSHAPES.path(dir), 1)?))
    }

    pub fn expression(&mut self, landmarks: &Landmarks, cancel: &AtomicBool) -> Result<Expression> {
        let input: Vec<f32> =
            BLENDSHAPE_LANDMARKS.iter().flat_map(|&i| [landmarks.points[i][0], landmarks.points[i][1]]).collect();
        let outputs = run(&mut self.0, &[1, BLENDSHAPE_LANDMARKS.len(), 2], &input, cancel)?;
        let s = &outputs.first().context("Expression model gave no output")?.data;
        anyhow::ensure!(s.len() == 52, "Unexpected expression output size {}", s.len());
        let smile = (s[MOUTH_SMILE_LEFT] + s[MOUTH_SMILE_RIGHT]) / 2.0;
        let looks = [EYE_LOOK_DOWN_LEFT, EYE_LOOK_IN_LEFT, EYE_LOOK_OUT_LEFT, EYE_LOOK_UP_LEFT];
        let look_away = looks.iter().flat_map(|&i| [s[i], s[i + 1]]).fold(0.0, f32::max);
        // Nose tip against the outer eye corners, along the eye line.
        let p = &landmarks.points;
        let (nose, right, left) = (p[1], p[33], p[263]);
        let (cos, sin) = (landmarks.roll.cos(), landmarks.roll.sin());
        let along = |q: [f32; 3]| q[0] * cos + q[1] * sin;
        let width = (along(left) - along(right)).abs().max(1e-3);
        let yaw = (along(nose) - (along(left) + along(right)) / 2.0) / width;
        let pair = |left: usize| (s[left] + s[left + 1]) / 2.0;
        let distance = |a: usize, b: usize| (p[a][0] - p[b][0]).hypot(p[a][1] - p[b][1]);
        let lips_apart =
            distance(UPPER_LIP_INNER, LOWER_LIP_INNER) / distance(MOUTH_INNER_RIGHT, MOUTH_INNER_LEFT).max(1e-3);
        Ok(Expression {
            blink: s[EYE_BLINK_LEFT].max(s[EYE_BLINK_RIGHT]),
            smile,
            lips_apart,
            purse: s[MOUTH_FUNNEL].max(s[MOUTH_PUCKER]),
            frown: [BROW_DOWN_LEFT, MOUTH_FROWN_LEFT, MOUTH_UPPER_UP_LEFT, NOSE_SNEER_LEFT]
                .map(pair)
                .into_iter()
                .fold(0.0, f32::max),
            look_away,
            yaw,
        })
    }
}

// Face Mesh landmarks of the inner lips: the middle of each and the mouth's inner corners.
const UPPER_LIP_INNER: usize = 13;
const LOWER_LIP_INNER: usize = 14;
const MOUTH_INNER_RIGHT: usize = 78;
const MOUTH_INNER_LEFT: usize = 308;

// Indices into MediaPipe's 52 blendshape categories; a left one is followed by its right twin.
const BROW_DOWN_LEFT: usize = 1;
const EYE_BLINK_LEFT: usize = 9;
const EYE_BLINK_RIGHT: usize = 10;
const EYE_LOOK_DOWN_LEFT: usize = 11;
const EYE_LOOK_IN_LEFT: usize = 13;
const EYE_LOOK_OUT_LEFT: usize = 15;
const EYE_LOOK_UP_LEFT: usize = 17;
const MOUTH_FUNNEL: usize = 32;
const MOUTH_PUCKER: usize = 38;
const MOUTH_SMILE_LEFT: usize = 44;
const MOUTH_SMILE_RIGHT: usize = 45;
const MOUTH_FROWN_LEFT: usize = 30;
const MOUTH_UPPER_UP_LEFT: usize = 48;
const NOSE_SNEER_LEFT: usize = 50;

/// The landmarks MediaPipe feeds the blendshape model, in its order (`kLandmarksSubsetIdxs` in
/// mediapipe/tasks/cc/vision/face_landmarker/face_blendshapes_graph.cc).
const BLENDSHAPE_LANDMARKS: [usize; 146] = [
    0, 1, 4, 5, 6, 7, 8, 10, 13, 14, 17, 21, 33, 37, 39, 40, 46, 52, 53, 54, 55, 58, 61, 63, 65, 66, 67, 70, 78, 80,
    81, 82, 84, 87, 88, 91, 93, 95, 103, 105, 107, 109, 127, 132, 133, 136, 144, 145, 146, 148, 149, 150, 152, 153,
    154, 155, 157, 158, 159, 160, 161, 162, 163, 168, 172, 173, 176, 178, 181, 185, 191, 195, 197, 234, 246, 249, 251,
    263, 267, 269, 270, 276, 282, 283, 284, 285, 288, 291, 293, 295, 296, 297, 300, 308, 310, 311, 312, 314, 317, 318,
    321, 323, 324, 332, 334, 336, 338, 356, 361, 362, 365, 373, 374, 375, 377, 378, 379, 380, 381, 382, 384, 385, 386,
    387, 388, 389, 390, 397, 398, 400, 402, 405, 409, 415, 454, 466, 468, 469, 470, 471, 472, 473, 474, 475, 476, 477,
];

/// MediaPipe Selfie Segmenter: the frame squeezed to 256², a person probability per pixel.
pub struct Selfie(Session);

const SELFIE_SIDE: usize = 256;
/// Smallest share of the frame that counts as a person rather than noise.
const SELFIE_MIN_SHARE: f32 = 0.005;

impl Selfie {
    pub fn load(dir: &Path) -> Result<Self> {
        Ok(Self(session(&crate::models::SELFIE.path(dir), 1)?))
    }

    /// Box around the person in the frame's pixels, or none.
    pub fn person(&mut self, frame: &Frame, cancel: &AtomicBool) -> Result<Option<Rect>> {
        let n = SELFIE_SIDE;
        let small = frame.resized(n as u32, n as u32);
        let input: Vec<f32> =
            small.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]].map(|c| c as f32 / 255.0)).collect();
        let outputs = run(&mut self.0, &[1, n, n, 3], &input, cancel)?;
        let mask = &outputs.first().context("Person model gave no output")?.data;
        anyhow::ensure!(mask.len() == n * n, "Unexpected person mask size {}", mask.len());
        let (mut x0, mut y0, mut x1, mut y1, mut count) = (n, n, 0, 0, 0usize);
        for (i, &p) in mask.iter().enumerate() {
            if p >= 0.5 {
                let (x, y) = (i % n, i / n);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                count += 1;
            }
        }
        if (count as f32) < SELFIE_MIN_SHARE * (n * n) as f32 {
            return Ok(None);
        }
        let (kx, ky) = (frame.width as f32 / n as f32, frame.height as f32 / n as f32);
        Ok(Some([x0 as f32 * kx, y0 as f32 * ky, (x1 - x0 + 1) as f32 * kx, (y1 - y0 + 1) as f32 * ky]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn still(name: &str) -> Frame {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp-test").join(name);
        let file = std::fs::File::open(&path)
            .unwrap_or_else(|_| panic!("{} is missing: run scripts/fixtures.sh", path.display()));
        let mut reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().unwrap();
        let mut rgb = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut rgb).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgb);
        let rgba = rgb.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        Frame { width: info.width, height: info.height, rgba }
    }

    /// The face models on two stills, without the renderer, so it also runs under an emulated CPU
    /// without AVX: scripts/check.sh runs it with `qemu-x86_64-static -cpu Nehalem`.
    #[test]
    #[ignore = "Requires tmp-test/face-open.png, face-blink.png and the models; run with XDG_DATA_HOME=$PWD/tmp-test/xdg/data"]
    fn the_face_models_tell_open_eyes_from_a_blink() {
        let dir = PathBuf::from(std::env::var_os("XDG_DATA_HOME").expect("XDG_DATA_HOME")).join("nuzky/models");
        let cancel = AtomicBool::new(false);
        let (mut yunet, mut mesh, mut blend) =
            (Yunet::load(&dir).unwrap(), FaceMesh::load(&dir).unwrap(), Blendshapes::load(&dir).unwrap());
        let mut blink = |name: &str| {
            let frame = still(name);
            let small = frame.fit(640);
            let k = frame.width as f32 / small.width as f32;
            let faces = yunet.detect(&small, &cancel).unwrap();
            assert_eq!(faces.len(), 1, "{name}");
            let landmarks = mesh.landmarks(&frame, &faces[0].scaled(k), &cancel).unwrap().expect("a face");
            blend.expression(&landmarks, &cancel).unwrap().blink
        };
        let (open, shut) = (blink("face-open.png"), blink("face-blink.png"));
        assert!(open < 0.3 && shut > 0.5, "open {open}, blink {shut}");
    }
}
