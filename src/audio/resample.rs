//! Offline Catmull-Rom resampler (used for tests and non-real-time conversions).
//! Real-time streams rely on WASAPI's built-in sample-rate converter instead.

/// Resample interleaved `input` with `channels` from `from_hz` to `to_hz`.
pub fn resample(input: &[f32], channels: usize, from_hz: f64, to_hz: f64) -> Vec<f32> {
    let ch = channels.max(1);
    let frames = input.len() / ch;
    if frames == 0 || from_hz <= 0.0 || to_hz <= 0.0 {
        return Vec::new();
    }
    let ratio = from_hz / to_hz;
    let out_frames = ((frames as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_frames * ch);
    let at = |f: isize, c: usize| -> f32 {
        let f = f.clamp(0, frames as isize - 1) as usize;
        input[f * ch + c]
    };
    for i in 0..out_frames {
        let pos = i as f64 * ratio;
        let base = pos.floor() as isize;
        let t = (pos - base as f64) as f32;
        for c in 0..ch {
            let (p0, p1, p2, p3) = (
                at(base - 1, c),
                at(base, c),
                at(base + 1, c),
                at(base + 2, c),
            );
            let v = p1
                + 0.5
                    * t
                    * (p2 - p0
                        + t * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3
                            + t * (3.0 * (p1 - p2) + p3 - p0)));
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn sine_preserved() {
        let x: Vec<f32> = (0..44_100)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 44_100.0).sin())
            .collect();
        let y = super::resample(&x, 1, 44_100.0, 48_000.0);
        assert_eq!(y.len(), 48_000);
        let err = (1000..47_000)
            .map(|i| {
                let want = (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 48_000.0).sin();
                (y[i] as f64 - want).abs()
            })
            .fold(0.0, f64::max);
        assert!(err < 1e-3, "{err}");
    }
}
