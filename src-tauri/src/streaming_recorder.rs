//! Streaming WAV recorder for record-only mode.
//!
//! Writes 16kHz mono PCM directly to disk via a bounded channel + writer
//! thread, so memory usage is independent of recording duration (meetings
//! can run for tens of minutes). The WAV header is written with placeholder
//! sizes at open and rewritten with real sizes on finalize; `fix_wav_header`
//! salvages files whose finalize never ran (crash / forced stop).

use crate::audio::{Resampler, TARGET_SAMPLE_RATE};
use crate::commands::data_management_cmd::is_valid_stem;
use crate::data_saving::{f32_to_i16_clamped, generate_timestamp_filename};
use crate::error::AppError;
use std::fs;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;
use tracing::{error, info, warn};

/// Samples per channel block (16kHz → 64ms per block).
const BLOCK_SAMPLES: usize = 1024;
/// Bounded channel capacity: 256 blocks ≈ 16s of audio ≈ 512KB memory cap.
const CHANNEL_CAPACITY: usize = 256;
/// Dropped-block count that triggers a controlled stop (≈3.2s of audio).
const DROP_STOP_THRESHOLD: u64 = 50;

const PLACEHOLDER_SIZE: u32 = u32::MAX;
const WAV_HEADER_LEN: u64 = 44;

/// Bounded wait for the writer thread during forced stops (watchdog / tray
/// reset / release). Exceeded → detach + next-startup salvage.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Outcome of a successful finalize.
pub struct FinalizeInfo {
    pub wav_path: PathBuf,
    pub stem: String,
    /// PCM payload size in bytes.
    pub data_size: u64,
    pub dropped_blocks: u64,
}

impl FinalizeInfo {
    /// Duration of the finalized recording in milliseconds (16kHz mono i16).
    pub fn duration_ms(&self) -> u32 {
        let samples = self.data_size / 2;
        (samples * 1000 / TARGET_SAMPLE_RATE as u64) as u32
    }
}

/// Whether the accumulated dropped-block count exceeds the controlled-stop
/// threshold. Single source of truth for the backpressure policy (used by
/// the record-only session layer).
pub fn exceeds_drop_threshold(dropped: u64) -> bool {
    dropped > DROP_STOP_THRESHOLD
}

/// Streaming recorder: resamples device-rate f32 input to 16kHz i16 blocks
/// and streams them to a writer thread over a bounded channel.
///
/// `push_samples` is called from the cpal audio callback; it never blocks
/// the callback on disk I/O — a full channel drops the block and increments
/// the shared counter (the session layer polls `dropped_blocks` /
/// `exceeds_drop_threshold` to trigger a controlled stop).
pub struct StreamingRecorder {
    sender: Option<mpsc::SyncSender<Vec<i16>>>,
    result_rx: Option<mpsc::Receiver<Result<u64, AppError>>>,
    writer_handle: Option<JoinHandle<()>>,
    resampler: Option<Resampler>,
    pending_block: Vec<i16>,
    dropped: Arc<AtomicU64>,
    wav_path: PathBuf,
    stem: String,
}

impl StreamingRecorder {
    /// Create the output file, write the placeholder header, and spawn the
    /// writer thread. Fails fast (before any audio flows) if the directory
    /// or file cannot be created.
    pub fn start(dir: &Path, device_sample_rate: u32) -> Result<Self, AppError> {
        fs::create_dir_all(dir)?;
        let stem = generate_timestamp_filename();
        let wav_path = dir.join(format!("{stem}.wav"));
        let file = fs::File::create(&wav_path)?;
        let mut writer = BufWriter::new(file);
        write_placeholder_header(&mut writer)?;
        // Flush immediately so crash salvage always finds a valid header.
        writer.flush()?;

        let (tx, rx) = mpsc::sync_channel::<Vec<i16>>(CHANNEL_CAPACITY);
        let (result_tx, result_rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("record-only-writer".to_string())
            .spawn(move || writer_main(writer, &rx, &result_tx))
            .map_err(AppError::Io)?;

        let resampler = (device_sample_rate != TARGET_SAMPLE_RATE)
            .then(|| Resampler::new(device_sample_rate, TARGET_SAMPLE_RATE));

        Ok(Self {
            sender: Some(tx),
            result_rx: Some(result_rx),
            writer_handle: Some(handle),
            resampler,
            pending_block: Vec::with_capacity(BLOCK_SAMPLES * 2),
            dropped: Arc::new(AtomicU64::new(0)),
            wav_path,
            stem,
        })
    }

    pub fn wav_path(&self) -> &Path {
        &self.wav_path
    }

    pub fn stem(&self) -> &str {
        &self.stem
    }

    pub fn dropped_blocks(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Shared dropped-block counter for the session layer's stop polling.
    pub fn dropped_counter(&self) -> Arc<AtomicU64> {
        self.dropped.clone()
    }

    /// Feed device-rate f32 samples (called from the audio callback).
    /// Resamples to 16kHz, converts to i16, and ships full blocks to the
    /// writer thread. Full channel → block dropped + counted.
    pub fn push_samples(&mut self, samples: &[f32]) {
        let Some(sender) = &self.sender else {
            return;
        };
        let resampled: &[f32] = match &mut self.resampler {
            Some(r) => r.process(samples),
            None => samples,
        };
        let pcm = f32_to_i16_clamped(resampled);
        self.pending_block.extend_from_slice(&pcm);
        while self.pending_block.len() >= BLOCK_SAMPLES {
            let block: Vec<i16> = self.pending_block.drain(..BLOCK_SAMPLES).collect();
            try_send_block(sender, block, &self.dropped);
        }
    }

    /// Normal stop: close the channel, wait for the writer to flush and
    /// rewrite the header, and return the finalize outcome.
    pub fn finalize(self) -> Result<FinalizeInfo, AppError> {
        self.shutdown(None)
    }

    /// Forced stop (watchdog / tray reset): like `finalize`, but the writer
    /// join is bounded by `timeout`. On timeout the writer thread is
    /// detached and the file is left for next-startup salvage.
    pub fn stop_and_wait(self, timeout: Duration) -> Result<FinalizeInfo, AppError> {
        self.shutdown(Some(timeout))
    }

    fn shutdown(mut self, timeout: Option<Duration>) -> Result<FinalizeInfo, AppError> {
        // Ship the pending tail (partial block) so no audio is lost.
        if let Some(sender) = &self.sender {
            if !self.pending_block.is_empty() {
                let tail = std::mem::take(&mut self.pending_block);
                try_send_block(sender, tail, &self.dropped);
            }
        }
        // Close the data channel so the writer sees EOF and finalizes.
        drop(self.sender.take());
        let Some(result_rx) = self.result_rx.take() else {
            return Err(AppError::Audio(
                "record-only recorder already shut down".to_string(),
            ));
        };

        let recv_result = match timeout {
            Some(t) => result_rx.recv_timeout(t).map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => ShutdownFailure::Timeout,
                mpsc::RecvTimeoutError::Disconnected => ShutdownFailure::Panicked,
            }),
            None => result_rx.recv().map_err(|_| ShutdownFailure::Panicked),
        };

        let data_size = match recv_result {
            Ok(Ok(size)) => size,
            Ok(Err(e)) => {
                error!("record-only writer reported I/O error: {e}");
                return Err(e);
            }
            Err(ShutdownFailure::Timeout) => {
                error!(
                    "record-only writer did not finish within {:?}; detaching, file left for startup salvage",
                    timeout.unwrap_or_default()
                );
                // Detach: drop the JoinHandle without joining.
                drop(self.writer_handle.take());
                return Err(AppError::Audio(
                    "record-only writer stop timed out".to_string(),
                ));
            }
            Err(ShutdownFailure::Panicked) => {
                error!("record-only writer thread panicked; attempting header salvage");
                if let Err(e) = fix_wav_header(&self.wav_path) {
                    error!("record-only header salvage failed: {e}");
                    let corrupt = self.wav_path.with_extension("corrupt");
                    if let Err(re) = fs::rename(&self.wav_path, &corrupt) {
                        error!("record-only rename to .corrupt failed: {re}");
                    }
                }
                return Err(AppError::Audio(
                    "record-only writer thread panicked".to_string(),
                ));
            }
        };

        // Writer sent its result as its last action; join to reap the thread.
        if let Some(handle) = self.writer_handle.take() {
            let _ = handle.join();
        }
        info!(
            "record-only recording finalized: {} ({} bytes, {} dropped blocks)",
            self.stem,
            data_size,
            self.dropped_blocks()
        );
        Ok(FinalizeInfo {
            wav_path: self.wav_path.clone(),
            stem: self.stem.clone(),
            data_size,
            dropped_blocks: self.dropped_blocks(),
        })
    }
}

enum ShutdownFailure {
    Timeout,
    Panicked,
}

/// Try to ship a block to the writer; on a full channel, drop it and count.
fn try_send_block(sender: &mpsc::SyncSender<Vec<i16>>, block: Vec<i16>, dropped: &AtomicU64) {
    if sender.try_send(block).is_err() {
        let n = dropped.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n % 10 == 0 {
            warn!("record-only writer backpressure: {n} block(s) dropped");
        }
    }
}

/// Writer thread body: writes blocks until the channel closes, then flushes
/// and rewrites the header. On a write error it drains the remaining channel
/// without writing, still attempts the header rewrite (best effort), and
/// reports the original error.
fn writer_main<W: Write + Seek>(
    mut w: W,
    rx: &mpsc::Receiver<Vec<i16>>,
    result_tx: &mpsc::Sender<Result<u64, AppError>>,
) {
    let mut data_size: u64 = 0;
    let mut write_failed: Option<AppError> = None;
    while let Ok(block) = rx.recv() {
        if write_failed.is_some() {
            continue;
        }
        match write_block(&mut w, &block) {
            Ok(n) => data_size += n,
            Err(e) => {
                error!("record-only writer: block write failed: {e}");
                write_failed = Some(e);
            }
        }
    }
    let header_result = finalize_header(&mut w, data_size);
    let result = match (write_failed, header_result) {
        (None, Ok(())) => Ok(data_size),
        (Some(e), _) => Err(e),
        (None, Err(e)) => Err(e),
    };
    let _ = result_tx.send(result);
}

fn write_block<W: Write>(w: &mut W, block: &[i16]) -> Result<u64, AppError> {
    let mut buf = Vec::with_capacity(block.len() * 2);
    for &s in block {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    w.write_all(&buf)?;
    Ok(buf.len() as u64)
}

/// Write the 44-byte WAV header with placeholder sizes (rewritten on finalize).
fn write_placeholder_header<W: Write>(w: &mut W) -> Result<(), AppError> {
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

/// Flush and rewrite the header's size fields with the real data size.
fn finalize_header<W: Write + Seek>(w: &mut W, data_size: u64) -> Result<(), AppError> {
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
pub fn fix_wav_header(path: &Path) -> Result<(), AppError> {
    let len = fs::metadata(path)?.len();
    if len < WAV_HEADER_LEN {
        return Err(AppError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("file too small for WAV header: {} bytes", len),
        )));
    }
    let data_size = (len - WAV_HEADER_LEN) as u32;
    let mut file = fs::OpenOptions::new().read(true).write(true).open(path)?;
    let mut magic = [0u8; 12];
    file.read_exact(&mut magic)?;
    if &magic[0..4] != b"RIFF" || &magic[8..12] != b"WAVE" {
        return Err(AppError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a RIFF/WAVE file",
        )));
    }
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&(36 + data_size).to_le_bytes())?;
    file.seek(SeekFrom::Start(40))?;
    file.write_all(&data_size.to_le_bytes())?;
    Ok(())
}

/// One-shot startup salvage: scan `dir` for record-only WAV files (valid
/// timestamp stems only) and fix their headers. Files that cannot be fixed
/// (e.g. 0 bytes, bad magic) are renamed to `.corrupt`. A missing or
/// unreadable directory is skipped silently. Never blocks startup.
pub fn salvage_incomplete_recordings(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wav") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_valid_stem(stem) {
            continue;
        }
        match fix_wav_header(&path) {
            Ok(()) => info!("salvage: fixed WAV header for {stem}"),
            Err(e) => {
                warn!("salvage: cannot fix {stem}: {e}; renaming to .corrupt");
                let corrupt = path.with_extension("corrupt");
                if let Err(re) = fs::rename(&path, &corrupt) {
                    warn!("salvage: rename to .corrupt failed for {stem}: {re}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dl-vt-stream-{name}"));
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
    fn test_start_writes_placeholder_header() {
        let dir = temp_dir("placeholder");
        let rec = StreamingRecorder::start(&dir, TARGET_SAMPLE_RATE);
        assert!(rec.is_ok());
        let rec = match rec {
            Ok(r) => r,
            Err(_) => return,
        };
        assert!(rec.wav_path().exists());
        assert!(is_valid_stem(rec.stem()));
        let bytes = fs::read(rec.wav_path());
        assert!(bytes.is_ok());
        let bytes = match bytes {
            Ok(b) => b,
            Err(_) => return,
        };
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(read_u32_le(&bytes, 4), PLACEHOLDER_SIZE);
        assert_eq!(read_u32_le(&bytes, 40), PLACEHOLDER_SIZE);
        assert_eq!(read_u32_le(&bytes, 24), TARGET_SAMPLE_RATE);
        assert!(rec.finalize().is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_finalize_rewrites_real_sizes() {
        let dir = temp_dir("finalize");
        let rec = StreamingRecorder::start(&dir, TARGET_SAMPLE_RATE);
        assert!(rec.is_ok());
        let mut rec = match rec {
            Ok(r) => r,
            Err(_) => return,
        };
        // 1 second of 16kHz audio.
        rec.push_samples(&vec![0.5f32; 16000]);
        let info = rec.finalize();
        assert!(info.is_ok());
        let info = match info {
            Ok(i) => i,
            Err(_) => return,
        };
        assert_eq!(info.data_size, 32000);
        assert_eq!(info.duration_ms(), 1000);
        assert_eq!(info.dropped_blocks, 0);
        let bytes = fs::read(&info.wav_path);
        assert!(bytes.is_ok());
        let bytes = match bytes {
            Ok(b) => b,
            Err(_) => return,
        };
        assert_eq!(bytes.len() as u64, WAV_HEADER_LEN + 32000);
        assert_eq!(read_u32_le(&bytes, 4), 36 + 32000);
        assert_eq!(read_u32_le(&bytes, 40), 32000);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_partial_block_flushed_on_finalize() {
        let dir = temp_dir("partial-block");
        let rec = StreamingRecorder::start(&dir, TARGET_SAMPLE_RATE);
        assert!(rec.is_ok());
        let mut rec = match rec {
            Ok(r) => r,
            Err(_) => return,
        };
        // 500 samples < BLOCK_SAMPLES: stays in pending_block until finalize,
        // which ships it as a partial block (no tail loss).
        rec.push_samples(&vec![0.1f32; 500]);
        let info = rec.finalize();
        assert!(info.is_ok());
        let info = match info {
            Ok(i) => i,
            Err(_) => return,
        };
        assert_eq!(info.data_size, 1000);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resampling_from_48k() {
        let dir = temp_dir("resample-48k");
        let rec = StreamingRecorder::start(&dir, 48_000);
        assert!(rec.is_ok());
        let mut rec = match rec {
            Ok(r) => r,
            Err(_) => return,
        };
        // 1 second at 48kHz → ~1 second at 16kHz.
        rec.push_samples(&vec![0.0f32; 48000]);
        let info = rec.finalize();
        assert!(info.is_ok());
        let info = match info {
            Ok(i) => i,
            Err(_) => return,
        };
        // Linear resampler output length ≈ 16000 samples (±1 block for the
        // pending tail that never ships).
        let samples = info.data_size / 2;
        assert!(samples > 16000 - BLOCK_SAMPLES as u64 * 2);
        assert!(samples <= 16000 + BLOCK_SAMPLES as u64);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_try_send_block_counts_drops_when_full() {
        let (tx, _rx) = mpsc::sync_channel::<Vec<i16>>(1);
        let dropped = AtomicU64::new(0);
        try_send_block(&tx, vec![1; BLOCK_SAMPLES], &dropped);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        // Channel full → drop + count.
        try_send_block(&tx, vec![2; BLOCK_SAMPLES], &dropped);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        try_send_block(&tx, vec![3; BLOCK_SAMPLES], &dropped);
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_drop_threshold_boundary() {
        assert!(!exceeds_drop_threshold(DROP_STOP_THRESHOLD));
        assert!(exceeds_drop_threshold(DROP_STOP_THRESHOLD + 1));
    }

    /// Writer that fails data writes but accepts header rewrites.
    struct FailOnDataWriter {
        inner: Cursor<Vec<u8>>,
        header_len: u64,
    }

    impl Write for FailOnDataWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            // Position ≥ header means a data (or post-seek header) write.
            // Fail only writes that start beyond the header region.
            if self.inner.position() >= self.header_len {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "simulated disk failure",
                ));
            }
            self.inner.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    impl Seek for FailOnDataWriter {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    #[test]
    fn test_writer_reports_error_but_fixes_header() {
        let mut w = FailOnDataWriter {
            inner: Cursor::new(Vec::new()),
            header_len: WAV_HEADER_LEN,
        };
        assert!(write_placeholder_header(&mut w).is_ok());
        let (tx, rx) = mpsc::sync_channel::<Vec<i16>>(CHANNEL_CAPACITY);
        let (result_tx, result_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || writer_main(w, &rx, &result_tx));
        assert!(tx.send(vec![1; BLOCK_SAMPLES]).is_ok());
        drop(tx);
        let result = result_rx.recv();
        assert!(result.is_ok());
        let result = match result {
            Ok(r) => r,
            Err(_) => return,
        };
        assert!(result.is_err());
        assert!(handle.join().is_ok());
    }

    #[test]
    fn test_fix_wav_header_repairs_placeholder_sizes() {
        let dir = temp_dir("fix-header");
        let path = dir.join("2026-08-18_10-00-00.wav");
        let mut bytes = Vec::new();
        {
            let mut c = Cursor::new(&mut bytes);
            assert!(write_placeholder_header(&mut c).is_ok());
        }
        // 1000 i16 samples = 2000 bytes of payload.
        bytes.extend_from_slice(&vec![0u8; 2000]);
        assert!(fs::write(&path, &bytes).is_ok());

        assert!(fix_wav_header(&path).is_ok());
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
        assert!(fix_wav_header(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_fix_wav_header_rejects_bad_magic() {
        let dir = temp_dir("fix-magic");
        let path = dir.join("2026-08-18_10-00-02.wav");
        assert!(fs::write(&path, [0u8; 100]).is_ok());
        assert!(fix_wav_header(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_salvage_fixes_valid_stem_wav() {
        let dir = temp_dir("salvage-fix");
        let path = dir.join("2026-08-18_10-00-03.wav");
        let mut bytes = Vec::new();
        {
            let mut c = Cursor::new(&mut bytes);
            assert!(write_placeholder_header(&mut c).is_ok());
        }
        bytes.extend_from_slice(&[0u8; 200]);
        assert!(fs::write(&path, &bytes).is_ok());

        salvage_incomplete_recordings(&dir);
        let fixed = fs::read(&path);
        assert!(fixed.is_ok());
        let fixed = match fixed {
            Ok(b) => b,
            Err(_) => return,
        };
        assert_eq!(read_u32_le(&fixed, 40), 200);
        assert!(!dir.join("2026-08-18_10-00-03.corrupt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_salvage_zero_byte_renamed_corrupt() {
        let dir = temp_dir("salvage-zero");
        let path = dir.join("2026-08-18_10-00-04.wav");
        assert!(fs::write(&path, []).is_ok());

        salvage_incomplete_recordings(&dir);
        assert!(!path.exists());
        assert!(dir.join("2026-08-18_10-00-04.corrupt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_salvage_skips_missing_dir() {
        let dir = std::env::temp_dir().join("dl-vt-stream-no-such-dir");
        let _ = fs::remove_dir_all(&dir);
        // Must not panic.
        salvage_incomplete_recordings(&dir);
        assert!(!dir.exists());
    }

    #[test]
    fn test_salvage_skips_non_stem_files() {
        let dir = temp_dir("salvage-nonstem");
        let path = dir.join("my-voice-memo.wav");
        assert!(fs::write(&path, []).is_ok());

        salvage_incomplete_recordings(&dir);
        // Untouched: not renamed, not deleted.
        assert!(path.exists());
        assert!(!dir.join("my-voice-memo.corrupt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_salvage_ignores_non_wav_files() {
        let dir = temp_dir("salvage-nonwav");
        let path = dir.join("2026-08-18_10-00-05.json");
        assert!(fs::write(&path, b"{}").is_ok());

        salvage_incomplete_recordings(&dir);
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_stop_and_wait_completes_normally() {
        let dir = temp_dir("stop-wait");
        let rec = StreamingRecorder::start(&dir, TARGET_SAMPLE_RATE);
        assert!(rec.is_ok());
        let mut rec = match rec {
            Ok(r) => r,
            Err(_) => return,
        };
        rec.push_samples(&vec![0.2f32; 8000]);
        let info = rec.stop_and_wait(Duration::from_secs(5));
        assert!(info.is_ok());
        let info = match info {
            Ok(i) => i,
            Err(_) => return,
        };
        assert_eq!(info.data_size, 16000);
        let _ = fs::remove_dir_all(&dir);
    }
}
