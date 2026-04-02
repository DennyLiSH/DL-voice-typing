use crate::config::AppConfig;
use crate::error::AppError;
use std::fs;
use std::path::PathBuf;

pub const TARGET_SAMPLE_RATE: u32 = 16000;

/// Result of saving audio data.
pub struct SaveResult {
    /// Path to the saved WAV file.
    pub wav_path: PathBuf,
    /// Path to the saved JSON metadata file.
    pub json_path: PathBuf,
}

/// Save raw audio samples as a 16kHz mono WAV file with a companion JSON metadata file.
/// The JSON initially has `transcription: null` — call `update_json_with_text()` after transcription.
pub fn save_audio(
    samples: &[f32],
    original_sample_rate: u32,
    config: &AppConfig,
) -> Result<SaveResult, AppError> {
    let save_dir = PathBuf::from(&config.data_saving_path);
    fs::create_dir_all(&save_dir)?;

    let filename = generate_timestamp_filename();
    let wav_path = save_dir.join(format!("{}.wav", filename));
    let json_path = save_dir.join(format!("{}.json", filename));

    // Resample to 16kHz if needed.
    let resampled = if original_sample_rate != TARGET_SAMPLE_RATE {
        resample(samples, original_sample_rate, TARGET_SAMPLE_RATE)
    } else {
        samples.to_vec()
    };

    // Convert f32 → i16 with clamping.
    let pcm_data = f32_to_i16_clamped(&resampled);

    // Write WAV file.
    write_wav(&wav_path, &pcm_data, TARGET_SAMPLE_RATE)?;

    // Write JSON metadata (transcription = null for now).
    let duration_seconds = resampled.len() as f64 / TARGET_SAMPLE_RATE as f64;
    let metadata = serde_json::json!({
        "timestamp": chrono_now_rfc3339(),
        "language": config.language,
        "whisper_model": config.whisper_model,
        "sample_rate": TARGET_SAMPLE_RATE,
        "original_sample_rate": original_sample_rate,
        "duration_seconds": (duration_seconds * 1000.0).round() / 1000.0,
        "transcription": serde_json::Value::Null,
        "llm_corrected": serde_json::Value::Null,
    });
    let json_content = serde_json::to_string_pretty(&metadata)?;
    fs::write(&json_path, json_content)?;

    Ok(SaveResult {
        wav_path,
        json_path,
    })
}

/// Update the JSON metadata file with transcription text.
pub fn update_json_with_text(
    json_path: &std::path::Path,
    transcription: &str,
    llm_corrected: Option<&str>,
    final_text: Option<&str>,
) -> Result<(), AppError> {
    let content = fs::read_to_string(json_path)?;
    let mut metadata: serde_json::Value = serde_json::from_str(&content)?;

    metadata["transcription"] = serde_json::Value::String(transcription.to_string());
    metadata["llm_corrected"] = match llm_corrected {
        Some(text) => serde_json::Value::String(text.to_string()),
        None => serde_json::Value::Null,
    };
    metadata["final_text"] = match final_text {
        Some(text) => serde_json::Value::String(text.to_string()),
        None => serde_json::Value::Null,
    };

    let updated = serde_json::to_string_pretty(&metadata)?;
    fs::write(json_path, updated)?;
    Ok(())
}

/// Linear interpolation resampling.
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = ((samples.len() as f64) / ratio).round() as usize;
    let mut output = Vec::with_capacity(output_len);
    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;
        let s0 = samples[src_idx];
        let s1 = if src_idx + 1 < samples.len() {
            samples[src_idx + 1]
        } else {
            s0
        };
        output.push((s0 as f64 + frac * (s1 as f64 - s0 as f64)) as f32);
    }
    output
}

/// Convert f32 samples to i16 with clamping. NaN/Inf → 0.
fn f32_to_i16_clamped(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| {
            if !s.is_finite() {
                return 0i16;
            }
            let clamped = s.clamp(-1.0, 1.0);
            (clamped * 32767.0) as i16
        })
        .collect()
}

/// Write a standard 16-bit mono PCM WAV file.
fn write_wav(path: &std::path::Path, pcm_data: &[i16], sample_rate: u32) -> Result<(), AppError> {
    let num_channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample / 8) as u32;
    let block_align = num_channels * (bits_per_sample / 8);
    let data_size = pcm_data.len() as u32 * (bits_per_sample / 8) as u32;
    let file_size = 36 + data_size; // RIFF header size minus 8

    let mut buf = Vec::with_capacity(44 + pcm_data.len() * 2);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt sub-chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // sub-chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    buf.extend_from_slice(&num_channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&bits_per_sample.to_le_bytes());

    // data sub-chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    for &sample in pcm_data {
        buf.extend_from_slice(&sample.to_le_bytes());
    }

    fs::write(path, buf)?;
    Ok(())
}

/// Generate a timestamp-based filename (e.g., "2026-04-02_14-30-25").
fn generate_timestamp_filename() -> String {
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // Simple formatting without chrono dependency.
    let total_secs = duration.as_secs();
    // Days since 1970-01-01 → convert to year/month/day
    let (year, month, day, hour, minute, second) = unix_time_to_date(total_secs);
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        year, month, day, hour, minute, second
    )
}

/// Convert Unix timestamp to (year, month, day, hour, minute, second).
/// Simplified algorithm — valid for 1970–2099.
fn unix_time_to_date(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hour = time_of_day / 3600;
    let minute = (time_of_day % 3600) / 60;
    let second = time_of_day % 60;

    // Calculate year from days since epoch.
    let mut year = 1970u64;
    let mut remaining_days = days_since_epoch;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    // Calculate month and day.
    let mut month = 1u64;
    let mut day = remaining_days;
    let days_in_months = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    for &dim in &days_in_months {
        if day < dim {
            break;
        }
        day -= dim;
        month += 1;
    }
    day += 1; // 1-indexed

    (year, month, day, hour, minute, second)
}

fn is_leap_year(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// RFC 3339 formatted timestamp using the local timezone offset.
fn chrono_now_rfc3339() -> String {
    // Without chrono, produce a basic ISO 8601-like string.
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = duration.as_secs();
    let (year, month, day, hour, minute, second) = unix_time_to_date(total_secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+08:00",
        year, month, day, hour, minute, second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_audio_happy_path() {
        let dir = std::env::temp_dir().join("dl-voice-typing-test-save-audio");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let config = AppConfig {
            data_saving_enabled: true,
            data_saving_path: dir.to_string_lossy().to_string(),
            ..Default::default()
        };

        // Generate 1 second of 16kHz sine wave.
        let samples: Vec<f32> = (0..16000)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 16000.0).sin() * 0.5)
            .collect();

        let result = save_audio(&samples, 16000, &config).unwrap();
        assert!(result.wav_path.exists());
        assert!(result.json_path.exists());

        // Verify WAV header starts with RIFF.
        let wav_bytes = fs::read(&result.wav_path).unwrap();
        assert_eq!(&wav_bytes[0..4], b"RIFF");
        assert_eq!(&wav_bytes[8..12], b"WAVE");

        // Verify JSON has null transcription.
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&result.json_path).unwrap()).unwrap();
        assert!(json["transcription"].is_null());
        assert_eq!(json["sample_rate"], 16000);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_audio_creates_directory() {
        let dir = std::env::temp_dir()
            .join("dl-voice-typing-test-mkdir")
            .join("subdir")
            .join("nested");
        let _ = fs::remove_dir_all(&dir.parent().unwrap().parent().unwrap());

        let config = AppConfig {
            data_saving_enabled: true,
            data_saving_path: dir.to_string_lossy().to_string(),
            ..Default::default()
        };

        let samples = vec![0.0f32; 100];
        let result = save_audio(&samples, 16000, &config).unwrap();
        assert!(dir.exists());
        assert!(result.wav_path.exists());

        let _ = fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn test_save_audio_invalid_path() {
        let config = AppConfig {
            data_saving_enabled: true,
            data_saving_path: "/nonexistent/deeply/nested/invalid\0path".to_string(),
            ..Default::default()
        };

        let samples = vec![0.0f32; 100];
        // Should return an error, not panic.
        assert!(save_audio(&samples, 16000, &config).is_err());
    }

    #[test]
    fn test_resample_48k_to_16k() {
        // 48000 Hz → 16000 Hz = 3:1 ratio.
        let samples: Vec<f32> = (0..48000).map(|i| i as f32).collect();
        let resampled = resample(&samples, 48000, 16000);
        assert_eq!(resampled.len(), 16000);
    }

    #[test]
    fn test_resample_identity() {
        let samples: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let resampled = resample(&samples, 16000, 16000);
        assert_eq!(resampled, samples);
    }

    #[test]
    fn test_f32_to_i16_clamping() {
        let samples = vec![2.0, -2.0, 0.5, -0.5, 0.0];
        let pcm = f32_to_i16_clamped(&samples);
        assert_eq!(pcm[0], 32767); // 2.0 clamped to max
        assert_eq!(pcm[1], -32767); // -2.0 clamped to min
        assert!((pcm[2] as i32 - 16383).abs() <= 1); // 0.5 ≈ 16383
        assert!((pcm[3] as i32 + 16383).abs() <= 1); // -0.5 ≈ -16383
        assert_eq!(pcm[4], 0);
    }

    #[test]
    fn test_f32_to_i16_nan_inf() {
        let samples = vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
        let pcm = f32_to_i16_clamped(&samples);
        assert_eq!(pcm, vec![0, 0, 0]);
    }

    #[test]
    fn test_wav_header_format() {
        let dir = std::env::temp_dir().join("dl-voice-typing-test-wav-header");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let path = dir.join("test.wav");
        let pcm = vec![0i16; 100];
        write_wav(&path, &pcm, 16000).unwrap();

        let bytes = fs::read(&path).unwrap();
        // RIFF header.
        assert_eq!(&bytes[0..4], b"RIFF");
        // File size = 36 + 200 (100 samples * 2 bytes).
        let file_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        assert_eq!(file_size, 236);
        // WAVE marker.
        assert_eq!(&bytes[8..12], b"WAVE");
        // fmt sub-chunk.
        assert_eq!(&bytes[12..16], b"fmt ");
        // PCM format.
        let audio_format = u16::from_le_bytes([bytes[20], bytes[21]]);
        assert_eq!(audio_format, 1);
        // Mono.
        let channels = u16::from_le_bytes([bytes[22], bytes[23]]);
        assert_eq!(channels, 1);
        // 16000 Hz.
        let sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
        assert_eq!(sample_rate, 16000);
        // 16 bits per sample.
        let bits = u16::from_le_bytes([bytes[34], bytes[35]]);
        assert_eq!(bits, 16);
        // data sub-chunk.
        assert_eq!(&bytes[36..40], b"data");
        let data_size = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
        assert_eq!(data_size, 200);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_update_json_with_text() {
        let dir = std::env::temp_dir().join("dl-voice-typing-test-update-json");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let json_path = dir.join("test.json");
        let initial = serde_json::json!({
            "transcription": serde_json::Value::Null,
            "llm_corrected": serde_json::Value::Null,
        });
        fs::write(&json_path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        update_json_with_text(&json_path, "你好世界", Some("你好世界"), None).unwrap();

        let updated: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
        assert_eq!(updated["transcription"], "你好世界");
        assert_eq!(updated["llm_corrected"], "你好世界");
        assert_eq!(updated["final_text"], serde_json::Value::Null);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_update_json_with_final_text() {
        let dir = std::env::temp_dir().join("dl-voice-typing-test-final-text");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let json_path = dir.join("test.json");
        let initial = serde_json::json!({
            "transcription": serde_json::Value::Null,
            "llm_corrected": serde_json::Value::Null,
        });
        fs::write(&json_path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        update_json_with_text(
            &json_path,
            "原始转录",
            Some("LLM纠正"),
            Some("编辑后最终文本"),
        )
        .unwrap();

        let updated: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
        assert_eq!(updated["transcription"], "原始转录");
        assert_eq!(updated["llm_corrected"], "LLM纠正");
        assert_eq!(updated["final_text"], "编辑后最终文本");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_update_json_cancelled_review() {
        let dir = std::env::temp_dir().join("dl-voice-typing-test-cancelled");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let json_path = dir.join("test.json");
        let initial = serde_json::json!({
            "transcription": serde_json::Value::Null,
            "llm_corrected": serde_json::Value::Null,
        });
        fs::write(&json_path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        // Cancel: final_text is None
        update_json_with_text(&json_path, "原始转录", None, None).unwrap();

        let updated: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
        assert_eq!(updated["transcription"], "原始转录");
        assert_eq!(updated["llm_corrected"], serde_json::Value::Null);
        assert_eq!(updated["final_text"], serde_json::Value::Null);

        let _ = fs::remove_dir_all(&dir);
    }
}
