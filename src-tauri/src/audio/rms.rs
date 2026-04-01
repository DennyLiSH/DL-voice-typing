/// RMS (Root Mean Square) calculation for audio waveform visualization.
use rand::Rng;

/// Waveform bar weights: [0.5, 0.8, 1.0, 0.75, 0.55]
pub const BAR_WEIGHTS: [f32; 5] = [0.5, 0.8, 1.0, 0.75, 0.55];

/// Maximum bar height in pixels.
pub const MAX_BAR_HEIGHT: f32 = 32.0;

/// Minimum bar height in pixels.
pub const MIN_BAR_HEIGHT: f32 = 4.0;

/// Random jitter amplitude (±4%).
const JITTER_AMPLITUDE: f32 = 0.04;

/// Calculate RMS (root mean square) of audio samples.
pub fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Calculate waveform bar heights from RMS value.
///
/// Returns 5 bar heights (in pixels), each weighted and with optional jitter.
/// Applies envelope smoothing with attack 40% and release 15%.
pub fn rms_to_bar_heights(rms: f32, prev_heights: &[f32; 5], jitter: bool) -> [f32; 5] {
    let mut rng = rand::rng();
    let mut heights = [0.0f32; 5];

    for (i, height) in heights.iter_mut().enumerate() {
        let weight = BAR_WEIGHTS[i];

        // Apply jitter
        let jitter_mult = if jitter {
            1.0 + rng.random_range(-JITTER_AMPLITUDE..JITTER_AMPLITUDE)
        } else {
            1.0
        };

        let raw = (rms * weight * jitter_mult * MAX_BAR_HEIGHT * 3.0)
            .clamp(MIN_BAR_HEIGHT, MAX_BAR_HEIGHT);

        // Envelope: fast attack (40%), slow release (15%)
        let prev = prev_heights[i];
        let attack_factor = 0.4;
        let release_factor = 0.15;

        if raw > prev {
            *height = prev + (raw - prev) * attack_factor;
        } else {
            *height = prev + (raw - prev) * release_factor;
        }

        *height = height.clamp(MIN_BAR_HEIGHT, MAX_BAR_HEIGHT);
    }

    heights
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rms_silence() {
        let rms = calculate_rms(&[0.0; 1024]);
        assert!((rms - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_rms_full_scale() {
        let rms = calculate_rms(&[1.0; 1024]);
        assert!((rms - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_rms_known_signal() {
        // Square wave: ±0.5
        let samples: Vec<f32> = (0..1024)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let rms = calculate_rms(&samples);
        assert!((rms - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_rms_empty() {
        let rms = calculate_rms(&[]);
        assert!((rms - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_bar_heights_clamped() {
        let prev = [MIN_BAR_HEIGHT; 5];
        let heights = rms_to_bar_heights(1.0, &prev, false);
        for h in &heights {
            assert!(*h >= MIN_BAR_HEIGHT);
            assert!(*h <= MAX_BAR_HEIGHT);
        }
    }

    #[test]
    fn test_bar_heights_silence() {
        let prev = [MIN_BAR_HEIGHT; 5];
        let heights = rms_to_bar_heights(0.0, &prev, false);
        for h in &heights {
            assert!(*h >= MIN_BAR_HEIGHT);
        }
    }

    #[test]
    fn test_bar_heights_with_jitter() {
        let prev = [MIN_BAR_HEIGHT; 5];
        let h1 = rms_to_bar_heights(0.5, &prev, true);
        let h2 = rms_to_bar_heights(0.5, &prev, true);
        // With jitter, two calls should produce different results (probabilistic)
        // We just verify they're in range
        for h in h1.iter().chain(h2.iter()) {
            assert!(*h >= MIN_BAR_HEIGHT);
            assert!(*h <= MAX_BAR_HEIGHT);
        }
    }

    #[test]
    fn test_bar_heights_weights() {
        let prev = [MIN_BAR_HEIGHT; 5];
        let heights = rms_to_bar_heights(0.3, &prev, false);
        // Center bar (index 2, weight 1.0) should be tallest
        assert!(heights[2] >= heights[0]); // weight 1.0 > 0.5
        assert!(heights[2] >= heights[1]); // weight 1.0 > 0.8
    }

    #[test]
    fn test_envelope_attack_faster_than_release() {
        let prev = [MIN_BAR_HEIGHT; 5];
        // Attack: going up
        let attack_heights = rms_to_bar_heights(1.0, &prev, false);
        // Release: going down
        let release_heights = rms_to_bar_heights(0.0, &attack_heights, false);
        // After attack, bars should be higher than after one release step
        for i in 0..5 {
            assert!(attack_heights[i] > release_heights[i] || attack_heights[i] == MIN_BAR_HEIGHT);
        }
    }
}
