//! WAV byte-format single source: canonical 44-byte header construction,
//! placeholder-header open, finalize size rewrite, and chunk-walk salvage.
//!
//! Every producer of recording WAV files (classic pipeline via
//! `data_saving::write_wav`, record-only streaming via `streaming_recorder`)
//! delegates its byte-format knowledge here, so a layout change happens in
//! exactly one place. All functions are byte-identical to the pre-extraction
//! inline implementations.
//!
//! Salvage design (also the doc for [`fix_truncated`]): the data chunk is
//! located by WALKING the RIFF chunk list instead of assuming the fixed
//! 44-byte layout — third-party WAVs (e.g. ffmpeg output with LIST/INFO
//! metadata) legitimately carry extra chunks before `data`, and blindly
//! rewriting offsets 4/40 corrupts them beyond ffprobe repair (E2E
//! 2026-09-12). Size fields are rewritten ONLY on true truncation (declared
//! data size exceeds the bytes actually present — the [`PLACEHOLDER_SIZE`]
//! sentinel our crash case writes always trips this); a file whose declared
//! sizes fit the actual length is left untouched, making salvage idempotent
//! and leaving consistent third-party WAVs byte-for-byte untouched.

use crate::audio::TARGET_SAMPLE_RATE;
use crate::error::AppError;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Canonical WAV header length in bytes: 12-byte RIFF descriptor + 8-byte
/// fmt header + 16-byte fmt body + 8-byte data header.
pub const WAV_HEADER_LEN: u64 = 44;

/// Placeholder size sentinel written at offsets 4 and 40 by
/// [`write_placeholder`] (rewritten on finalize). `u32::MAX` always trips
/// the true-truncation check in [`fix_truncated`], so a file whose finalize
/// never ran is always salvageable.
pub const PLACEHOLDER_SIZE: u32 = u32::MAX;

/// Outcome of [`fix_truncated`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderFix {
    /// Size fields were rewritten — the declared data size exceeded the
    /// bytes actually present (crash-truncated recording with the
    /// `u32::MAX` placeholder, or a truncated third-party WAV).
    Fixed,
    /// Declared sizes fit the actual file length — nothing written. Makes
    /// re-runs idempotent and leaves third-party WAVs (LIST/INFO chunks,
    /// non-44-byte layouts) byte-for-byte untouched.
    AlreadyConsistent,
}

/// Write the canonical 44-byte RIFF/WAVE PCM header for a payload of
/// `samples_len` samples (`bits_per_sample`/8 bytes each), followed by the
/// caller's PCM payload. Byte-identical to the header previously inlined in
/// `data_saving::write_wav`.
pub fn write_full_header<W: Write>(
    w: &mut W,
    samples_len: u32,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
) -> Result<(), AppError> {
    let byte_rate = sample_rate * channels as u32 * (bits_per_sample / 8) as u32;
    let block_align = channels * (bits_per_sample / 8);
    let data_size = samples_len * (bits_per_sample / 8) as u32;
    let file_size = 36 + data_size;

    // RIFF header
    w.write_all(b"RIFF")?;
    w.write_all(&file_size.to_le_bytes())?;
    w.write_all(b"WAVE")?;

    // fmt sub-chunk
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&bits_per_sample.to_le_bytes())?;

    // data sub-chunk header
    w.write_all(b"data")?;
    w.write_all(&data_size.to_le_bytes())?;

    Ok(())
}

/// Write the 44-byte WAV header with placeholder sizes (rewritten by
/// [`finalize`]). 16kHz mono 16-bit PCM at [`TARGET_SAMPLE_RATE`].
pub fn write_placeholder<W: Write>(w: &mut W) -> Result<(), AppError> {
    w.write_all(b"RIFF")?;
    w.write_all(&PLACEHOLDER_SIZE.to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?; // PCM
    w.write_all(&1u16.to_le_bytes())?; // mono
    w.write_all(&TARGET_SAMPLE_RATE.to_le_bytes())?;
    w.write_all(&(TARGET_SAMPLE_RATE * 2).to_le_bytes())?; // byte rate
    w.write_all(&2u16.to_le_bytes())?; // block align
    w.write_all(&16u16.to_le_bytes())?; // bits per sample
    w.write_all(b"data")?;
    w.write_all(&PLACEHOLDER_SIZE.to_le_bytes())?;
    Ok(())
}

/// Flush and rewrite the header's size fields with the real data size
/// (RIFF size at offset 4, data size at offset 40).
pub fn finalize<W: Write + Seek>(w: &mut W, data_size: u64) -> Result<(), AppError> {
    w.flush()?;
    w.seek(SeekFrom::Start(4))?;
    w.write_all(&((36 + data_size) as u32).to_le_bytes())?;
    w.seek(SeekFrom::Start(40))?;
    w.write_all(&(data_size as u32).to_le_bytes())?;
    w.flush()?;
    Ok(())
}

/// Rewrite a WAV file's header sizes from the actual file length.
/// Used to salvage recordings whose finalize never ran (crash, forced stop).
///
/// The data chunk is located by WALKING the RIFF chunk list instead of
/// assuming the fixed 44-byte layout: third-party WAVs (e.g. ffmpeg output
/// with LIST/INFO metadata) legitimately carry extra chunks before `data`,
/// and blindly rewriting offsets 4/40 corrupts them beyond ffprobe repair
/// (E2E 2026-09-12). Size fields are rewritten ONLY when the declared data
/// size exceeds the bytes actually present (true truncation — the
/// `u32::MAX` placeholder our crash case writes always trips this); a file
/// whose declared sizes fit the actual length is left untouched.
pub fn fix_truncated(path: &Path) -> Result<HeaderFix, AppError> {
    let len = fs::metadata(path)?.len();
    if len < WAV_HEADER_LEN {
        return Err(AppError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("file too small for WAV header: {len} bytes"),
        )));
    }
    let mut file = fs::OpenOptions::new().read(true).write(true).open(path)?;
    let mut magic = [0u8; 12];
    file.read_exact(&mut magic)?;
    if &magic[0..4] != b"RIFF" || &magic[8..12] != b"WAVE" {
        return Err(AppError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a RIFF/WAVE file",
        )));
    }
    // Walk the chunk list from offset 12 to find the data chunk.
    let mut off: u64 = 12;
    while off + 8 <= len {
        let mut hdr = [0u8; 8];
        file.seek(SeekFrom::Start(off))?;
        file.read_exact(&mut hdr)?;
        let sz = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as u64;
        let body = off + 8;
        if &hdr[0..4] == b"data" {
            let available = len.saturating_sub(body);
            if sz <= available {
                return Ok(HeaderFix::AlreadyConsistent);
            }
            let real = available.min(u32::MAX as u64) as u32;
            file.seek(SeekFrom::Start(off + 4))?;
            file.write_all(&real.to_le_bytes())?;
            let riff = (len - 8).min(u32::MAX as u64) as u32;
            file.seek(SeekFrom::Start(4))?;
            file.write_all(&riff.to_le_bytes())?;
            return Ok(HeaderFix::Fixed);
        }
        // RIFF chunks are word-aligned: an odd body size carries one pad byte.
        off = body.saturating_add(sz).saturating_add(sz & 1);
    }
    Err(AppError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "no data chunk found",
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dl-vt-wav-{name}"));
        let _ = fs::remove_dir_all(&dir);
        assert!(fs::create_dir_all(&dir).is_ok());
        dir
    }

    fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }

    #[test]
    fn test_fix_wav_header_repairs_placeholder_sizes() {
        let dir = temp_dir("fix-header");
        let path = dir.join("2026-08-18_10-00-00.wav");
        let mut bytes = Vec::new();
        {
            let mut c = Cursor::new(&mut bytes);
            assert!(write_placeholder(&mut c).is_ok());
        }
        // 1000 i16 samples = 2000 bytes of payload.
        bytes.extend_from_slice(&vec![0u8; 2000]);
        assert!(fs::write(&path, &bytes).is_ok());

        assert!(fix_truncated(&path).is_ok());
        let fixed = fs::read(&path);
        assert!(fixed.is_ok());
        let fixed = match fixed {
            Ok(b) => b,
            Err(_) => return,
        };
        assert_eq!(read_u32_le(&fixed, 4), 36 + 2000);
        assert_eq!(read_u32_le(&fixed, 40), 2000);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_fix_wav_header_rejects_zero_byte() {
        let dir = temp_dir("fix-zero");
        let path = dir.join("2026-08-18_10-00-01.wav");
        assert!(fs::write(&path, []).is_ok());
        assert!(fix_truncated(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_fix_wav_header_rejects_bad_magic() {
        let dir = temp_dir("fix-magic");
        let path = dir.join("2026-08-18_10-00-02.wav");
        assert!(fs::write(&path, [0u8; 100]).is_ok());
        assert!(fix_truncated(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    /// ffmpeg-style layout: fmt + LIST/INFO chunk before data, with the data
    /// chunk therefore NOT at the fixed 44-byte offset. `listed_len` bytes of
    /// audio payload are appended and the data size field is set to
    /// `declared_data` (callers pass a larger value to simulate truncation,
    /// or the true length for a consistent file).
    fn build_list_wav(listed_len: usize, declared_data: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&[0u8; 4]); // patched at the end
        bytes.extend_from_slice(b"WAVE");
        // fmt chunk (16-byte PCM mono 16kHz 16-bit)
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&16_000u32.to_le_bytes());
        bytes.extend_from_slice(&32_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        // LIST chunk with a 26-byte INFO payload (even size → no pad byte)
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&26u32.to_le_bytes());
        bytes.extend_from_slice(b"INFOISFT0123456789abcdefgh");
        // data chunk
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&declared_data.to_le_bytes());
        bytes.extend_from_slice(&vec![7u8; listed_len]);
        let riff = (bytes.len() as u32 - 8).to_le_bytes();
        bytes[4..8].copy_from_slice(&riff);
        bytes
    }

    /// Third-party WAV with LIST/INFO chunks and CONSISTENT sizes must be
    /// left byte-for-byte untouched. The old fixed-44 implementation rewrote
    /// offsets 4/40 unconditionally — offset 40 lands inside the LIST body
    /// and corrupted ffmpeg-produced files beyond ffprobe repair (E2E
    /// 2026-09-12).
    #[test]
    fn test_fix_wav_header_leaves_third_party_list_wav_untouched() {
        let dir = temp_dir("fix-list");
        let path = dir.join("2026-08-18_10-00-10.wav");
        let bytes = build_list_wav(100, 100);
        assert!(fs::write(&path, &bytes).is_ok());

        assert!(matches!(
            fix_truncated(&path),
            Ok(HeaderFix::AlreadyConsistent)
        ));
        let after = fs::read(&path);
        assert!(after.is_ok());
        let after = match after {
            Ok(b) => b,
            Err(_) => return,
        };
        assert_eq!(after, bytes, "third-party WAV must stay byte-identical");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A genuinely truncated third-party WAV (declared data > bytes present,
    /// data chunk NOT at offset 40) is salvaged at the CORRECT offsets: the
    /// data size field inside the real data chunk header, the RIFF size at
    /// 4, and the fmt/LIST bytes untouched.
    #[test]
    fn test_fix_wav_header_repairs_truncated_third_party_wav_at_data_offset() {
        let dir = temp_dir("fix-list-trunc");
        let path = dir.join("2026-08-18_10-00-11.wav");
        let bytes = build_list_wav(400, 1000); // declared 1000, only 400 present
        assert!(fs::write(&path, &bytes).is_ok());

        assert!(matches!(fix_truncated(&path), Ok(HeaderFix::Fixed)));
        let fixed = fs::read(&path);
        assert!(fixed.is_ok());
        let fixed = match fixed {
            Ok(b) => b,
            Err(_) => return,
        };
        // The data chunk starts after fmt(12+8+16=36) + LIST(8+26=34) →
        // header at 70, size field at 74 — NOT the legacy fixed offset 40.
        assert_eq!(read_u32_le(&fixed, 74), 400);
        assert_eq!(read_u32_le(&fixed, 4), (fixed.len() - 8) as u32);
        // fmt + LIST region untouched.
        assert_eq!(&fixed[12..70], &bytes[12..70]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Idempotency: a file whose sizes are already consistent is reported
    /// as such and never rewritten (the old design rewrote every WAV header
    /// on every startup).
    #[test]
    fn test_fix_wav_header_consistent_file_reports_no_write() {
        let dir = temp_dir("fix-idempotent");
        let path = dir.join("2026-08-18_10-00-12.wav");
        let mut bytes = Vec::new();
        {
            let mut c = Cursor::new(&mut bytes);
            assert!(write_placeholder(&mut c).is_ok());
        }
        bytes.extend_from_slice(&vec![0u8; 2000]);
        assert!(fs::write(&path, &bytes).is_ok());

        assert!(matches!(fix_truncated(&path), Ok(HeaderFix::Fixed)));
        let after_first = fs::read(&path);
        assert!(after_first.is_ok());
        let after_first = match after_first {
            Ok(b) => b,
            Err(_) => return,
        };
        assert!(matches!(
            fix_truncated(&path),
            Ok(HeaderFix::AlreadyConsistent)
        ));
        let after_second = fs::read(&path);
        assert!(after_second.is_ok());
        let after_second = match after_second {
            Ok(b) => b,
            Err(_) => return,
        };
        assert_eq!(after_second, after_first);
        let _ = fs::remove_dir_all(&dir);
    }
}
