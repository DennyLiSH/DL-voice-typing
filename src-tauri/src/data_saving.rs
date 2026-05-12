use crate::audio::{TARGET_SAMPLE_RATE, resample};
use crate::config::AppConfig;
use crate::error::AppError;
use std::fs;
use std::path::PathBuf;

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
    let wav_path = save_dir.join(format!("{filename}.wav"));
    let json_path = save_dir.join(format!("{filename}.json"));

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
        "timestamp": now_rfc3339(),
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

/// Write a standard 16-bit mono PCM WAV file using streaming I/O.
fn write_wav(path: &std::path::Path, pcm_data: &[i16], sample_rate: u32) -> Result<(), AppError> {
    use std::io::Write;

    let num_channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample / 8) as u32;
    let block_align = num_channels * (bits_per_sample / 8);
    let data_size = pcm_data.len() as u32 * (bits_per_sample / 8) as u32;
    let file_size = 36 + data_size;

    let file = std::fs::File::create(path)?;
    let mut w = std::io::BufWriter::new(file);

    // RIFF header
    w.write_all(b"RIFF")?;
    w.write_all(&file_size.to_le_bytes())?;
    w.write_all(b"WAVE")?;

    // fmt sub-chunk
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&num_channels.to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&bits_per_sample.to_le_bytes())?;

    // data sub-chunk header
    w.write_all(b"data")?;
    w.write_all(&data_size.to_le_bytes())?;

    // Stream PCM samples in 8KB chunks.
    const CHUNK_SAMPLES: usize = 4096;
    for chunk in pcm_data.chunks(CHUNK_SAMPLES) {
        let mut buf = [0u8; CHUNK_SAMPLES * 2];
        for (i, &sample) in chunk.iter().enumerate() {
            let le = sample.to_le_bytes();
            buf[i * 2] = le[0];
            buf[i * 2 + 1] = le[1];
        }
        w.write_all(&buf[..chunk.len() * 2])?;
    }

    Ok(())
}

/// Generate a timestamp-based filename (e.g., "2026-04-02_14-30-25").
fn generate_timestamp_filename() -> String {
    use time::format_description::well_known::Rfc3339;
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    // Format as "YYYY-MM-DD_HH-MM-SS" for filename safety.
    let format = time::format_description::parse("[year]-[month]-[day]_[hour]-[minute]-[second]")
        .unwrap_or_else(|_| time::format_description::parse("[year]-[month]-[day]").unwrap());
    now.format(&format)
        .unwrap_or_else(|_| now.format(&Rfc3339).unwrap())
}

/// RFC 3339 formatted timestamp using the local timezone offset.
fn now_rfc3339() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .format(&Rfc3339)
        .unwrap_or_default()
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
        let _ = fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());

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
