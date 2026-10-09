//! Rendered RGBA frames and the few measures and resamplings the models need.

/// Straight RGBA, as the renderer returns it.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A rectangle `[x, y, width, height]` in pixels.
pub type Rect = [f32; 4];

impl Frame {
    /// Rec. 709 luma in 0..1, one value per pixel.
    pub fn luma(&self) -> Vec<f32> {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0)
            .collect()
    }

    /// Bilinear RGB in 0..1 at a point in pixel coordinates; outside the frame is black.
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let (w, h) = (self.width as i64, self.height as i64);
        let (x, y) = (x - 0.5, y - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let mut out = [0.0; 3];
        for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
            for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
                let (px, py) = (x0 + dx, y0 + dy);
                if px < 0 || py < 0 || px >= w || py >= h {
                    continue;
                }
                let i = ((py * w + px) * 4) as usize;
                let weight = wx * wy / 255.0;
                for (sum, &v) in out.iter_mut().zip(&self.rgba[i..i + 3]) {
                    *sum += v as f32 * weight;
                }
            }
        }
        out
    }

    /// Area-averaged copy at `width`×`height`: shrinking averages every source pixel it covers, so
    /// a small copy does not alias the way point sampling would.
    pub fn resized(&self, width: u32, height: u32) -> Frame {
        if (width, height) == (self.width, self.height) {
            return Frame { width, height, rgba: self.rgba.clone() };
        }
        let (sx, sy) = (self.width as f32 / width as f32, self.height as f32 / height as f32);
        if sx <= 1.0 && sy <= 1.0 {
            let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
            for y in 0..height {
                for x in 0..width {
                    let p = self.sample((x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
                    rgba.extend(p.map(|v| (v * 255.0).round() as u8));
                    rgba.push(255);
                }
            }
            return Frame { width, height, rgba };
        }
        // Rows and columns each take a weighted run of source pixels.
        let spans = |out: u32, scale: f32, max: u32| -> Vec<(usize, Vec<f32>)> {
            (0..out)
                .map(|o| {
                    let (a, b) = (o as f32 * scale, ((o + 1) as f32 * scale).min(max as f32));
                    let first = a.floor() as usize;
                    let weights: Vec<f32> =
                        (first..b.ceil() as usize).map(|i| ((i + 1) as f32).min(b) - (i as f32).max(a)).collect();
                    let total: f32 = weights.iter().sum();
                    (first, weights.into_iter().map(|w| w / total).collect())
                })
                .collect()
        };
        let (cols, rows) = (spans(width, sx, self.width), spans(height, sy, self.height));
        let stride = self.width as usize * 4;
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        let mut line = vec![0f32; self.width as usize * 3];
        for (y, (first_row, row_weights)) in rows.iter().enumerate() {
            line.fill(0.0);
            for (r, weight) in row_weights.iter().enumerate() {
                let row = &self.rgba[(first_row + r) * stride..][..stride];
                for (x, p) in row.as_chunks::<4>().0.iter().enumerate() {
                    for c in 0..3 {
                        line[x * 3 + c] += p[c] as f32 * weight;
                    }
                }
            }
            for (x, (first_col, col_weights)) in cols.iter().enumerate() {
                let mut p = [0f32; 3];
                for (i, weight) in col_weights.iter().enumerate() {
                    for c in 0..3 {
                        p[c] += line[(first_col + i) * 3 + c] * weight;
                    }
                }
                let o = (y * width as usize + x) * 4;
                rgba[o..o + 3].copy_from_slice(&p.map(|v| v.round().clamp(0.0, 255.0) as u8));
                rgba[o + 3] = 255;
            }
        }
        Frame { width, height, rgba }
    }

    /// A copy whose long side is at most `side`.
    pub fn fit(&self, side: u32) -> Frame {
        let long = self.width.max(self.height);
        if long <= side {
            return self.resized(self.width, self.height);
        }
        let k = side as f32 / long as f32;
        self.resized(((self.width as f32 * k).round() as u32).max(1), ((self.height as f32 * k).round() as u32).max(1))
    }
}

/// Variance of the 3×3 Laplacian of luma inside `rect`, the usual focus measure: a sharp picture has
/// strong local contrast, a blurred or shaken one has little. Scaled to 8-bit luma steps.
pub fn sharpness(luma: &[f32], width: u32, height: u32, rect: Rect) -> f32 {
    let (w, h) = (width as i64, height as i64);
    let x0 = (rect[0].floor() as i64).clamp(1, w - 1);
    let y0 = (rect[1].floor() as i64).clamp(1, h - 1);
    let x1 = ((rect[0] + rect[2]).ceil() as i64).clamp(1, w - 1);
    let y1 = ((rect[1] + rect[3]).ceil() as i64).clamp(1, h - 1);
    let (mut sum, mut squares, mut n) = (0f64, 0f64, 0f64);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * w + x) as usize;
            let w = w as usize;
            let v = 4.0 * luma[i] - luma[i - 1] - luma[i + 1] - luma[i - w] - luma[i + w];
            let v = v as f64 * 255.0;
            sum += v;
            squares += v * v;
            n += 1.0;
        }
    }
    if n < 1.0 {
        return 0.0;
    }
    let mean = sum / n;
    (squares / n - mean * mean).max(0.0) as f32
}

/// 1 for a well exposed picture, less as it gets dark or bright on average or loses detail to
/// crushed shadows and blown highlights.
pub fn exposure(luma: &[f32]) -> f32 {
    if luma.is_empty() {
        return 0.0;
    }
    let mean = luma.iter().sum::<f32>() / luma.len() as f32;
    let clipped = luma.iter().filter(|&&v| !(0.02..=0.98).contains(&v)).count() as f32 / luma.len() as f32;
    let level = 1.0 - ((mean - 0.5).abs() * 2.0).powi(2);
    (level * (1.0 - 2.0 * clipped)).clamp(0.0, 1.0)
}

/// Share of `inner` that lies inside `outer`.
pub fn inside(inner: Rect, outer: Rect) -> f32 {
    let area = inner[2] * inner[3];
    if area <= 0.0 {
        return 0.0;
    }
    let w = (inner[0] + inner[2]).min(outer[0] + outer[2]) - inner[0].max(outer[0]);
    let h = (inner[1] + inner[3]).min(outer[1] + outer[3]) - inner[1].max(outer[1]);
    (w.max(0.0) * h.max(0.0) / area).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(width: u32, height: u32, cell: u32) -> Frame {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let v = if (x / cell + y / cell).is_multiple_of(2) { 255 } else { 0 };
                rgba.extend([v, v, v, 255]);
            }
        }
        Frame { width, height, rgba }
    }

    #[test]
    fn shrinking_averages_instead_of_aliasing() {
        // A one-pixel checkerboard halved is flat grey; point sampling would keep black or white.
        let small = checker(64, 32, 1).resized(32, 16);
        assert!(small.rgba.as_chunks::<4>().0.iter().all(|p| (126..=129).contains(&p[0])), "{:?}", &small.rgba[..8]);
        // A third of a pixel: weights cover fractional source pixels too.
        let third = checker(96, 96, 32).resized(3, 3);
        assert_eq!(
            third.rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect::<Vec<_>>(),
            [255, 0, 255, 0, 255, 0, 255, 0, 255]
        );
    }

    #[test]
    fn blur_lowers_sharpness_and_flat_frames_have_none() {
        let sharp = checker(64, 64, 2);
        let blurred = sharp.resized(16, 16).resized(64, 64);
        let rect = [0.0, 0.0, 64.0, 64.0];
        let s = sharpness(&sharp.luma(), 64, 64, rect);
        assert!(s > 10.0 * sharpness(&blurred.luma(), 64, 64, rect), "{s}");
        assert_eq!(sharpness(&vec![0.5; 64 * 64], 64, 64, rect), 0.0);
    }

    #[test]
    fn exposure_prefers_mid_grey_over_black_and_white() {
        assert!(exposure(&[0.5; 100]) > 0.99);
        assert_eq!(exposure(&[0.0; 100]), 0.0);
        assert_eq!(exposure(&[1.0; 100]), 0.0);
        assert!(exposure(&[0.3; 100]) > exposure(&[0.15; 100]));
    }

    #[test]
    fn inside_measures_overlap_share() {
        assert_eq!(inside([0.0, 0.0, 10.0, 10.0], [5.0, 0.0, 100.0, 100.0]), 0.5);
        assert_eq!(inside([0.0, 0.0, 10.0, 10.0], [20.0, 20.0, 5.0, 5.0]), 0.0);
    }
}
