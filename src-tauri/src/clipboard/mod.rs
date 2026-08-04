use crate::error::AppError;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(50);
const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(2);

/// Execute a clipboard operation on a dedicated thread with a timeout.
/// Windows OpenClipboard blocks indefinitely if another process holds the clipboard.
fn with_clipboard_timeout<T>(
    operation: impl FnOnce() -> Result<T, AppError> + Send + 'static,
    timeout: Duration,
    label: &'static str,
) -> Result<T, AppError>
where
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = operation();
        let _ = tx.send(result);
    });
    rx.recv_timeout(timeout)
        .map_err(|_| AppError::Clipboard(format!("{label} timed out after {timeout:?}")))?
}

/// Trait for clipboard operations, enabling test seams.
/// All methods take `&self`; implementations use interior mutability so the
/// canonical handle is `Arc<dyn ClipboardProvider>` (matches `Arc<dyn SpeechEngine>`).
///
/// `save_and_inject` is the atomic delivery unit: implementations hold an
/// internal op lock spanning save → inject → self-restore, preserving the
/// cross-call atomicity the old outer `Mutex<AnyClipboard>` provided. Without
/// it, a Tray Reset `restore()` could interleave mid-injection and leak
/// transcription text into the user's clipboard.
pub trait ClipboardProvider: Send + Sync {
    fn save(&self) -> Result<(), AppError>;
    fn inject_text(&self, text: &str) -> Result<(), AppError>;
    /// Save current clipboard, inject `text` via paste, restore saved content —
    /// serialized against every other operation.
    fn save_and_inject(&self, text: &str) -> Result<(), AppError>;
    fn restore(&self) -> Result<(), AppError>;
    /// Whether `save()` has been called and not yet cleared by `restore()`.
    /// Used by panic-recovery (`RecordingSession::recover`) to decide whether
    /// restoring the old clipboard is meaningful: restore only when a save
    /// happened this cycle. A racing `restore()` between `was_saved()` and a
    /// subsequent `restore()` is benign: the second `restore()` finds
    /// `saved_content` already taken and no-ops.
    fn was_saved(&self) -> bool;
}

/// Clipboard manager for save/restore + Ctrl+V simulation.
pub struct ClipboardManager {
    saved_content: Mutex<Option<String>>,
    /// Serializes whole clipboard operations (save / inject / restore /
    /// save_and_inject) against each other — replaces the old outer mutex.
    op: Mutex<()>,
}

impl ClipboardManager {
    /// Create a new clipboard manager with no saved content.
    pub fn new() -> Self {
        Self {
            saved_content: Mutex::new(None),
            op: Mutex::new(()),
        }
    }

    fn save_inner(&self) -> Result<(), AppError> {
        let content = read_clipboard().ok();
        if let Some(mut guard) =
            crate::util::lock_mutex(&self.saved_content, "ClipboardManager::saved_content")
        {
            *guard = content;
        }
        Ok(())
    }

    fn inject_inner(&self, text: &str) -> Result<(), AppError> {
        write_clipboard_with_retry(text)?;
        simulate_paste()?;
        // Wait for target application to process the paste.
        std::thread::sleep(Duration::from_millis(80));
        let deadline = Instant::now() + Duration::from_millis(120);
        while Instant::now() < deadline {
            if read_clipboard().map_or(true, |c| c != text) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let saved = crate::util::lock_mutex(&self.saved_content, "ClipboardManager::saved_content")
            .and_then(|guard| guard.clone());
        if let Some(saved) = saved {
            let _ = write_clipboard_with_retry(&saved);
        }
        Ok(())
    }

    fn restore_inner(&self) -> Result<(), AppError> {
        let saved = crate::util::lock_mutex(&self.saved_content, "ClipboardManager::saved_content")
            .and_then(|mut guard| guard.take());
        if let Some(saved) = saved {
            write_clipboard_with_retry(&saved)?;
        }
        Ok(())
    }

    fn lock_op(&self) -> Result<std::sync::MutexGuard<'_, ()>, AppError> {
        crate::util::lock_mutex(&self.op, "ClipboardManager::op")
            .ok_or_else(|| AppError::Clipboard("op lock poisoned".to_string()))
    }
}

impl ClipboardProvider for ClipboardManager {
    fn save(&self) -> Result<(), AppError> {
        let _op = self.lock_op()?;
        self.save_inner()
    }

    fn inject_text(&self, text: &str) -> Result<(), AppError> {
        let _op = self.lock_op()?;
        self.inject_inner(text)
    }

    fn save_and_inject(&self, text: &str) -> Result<(), AppError> {
        let _op = self.lock_op()?;
        self.save_inner()?;
        self.inject_inner(text)
    }

    fn restore(&self) -> Result<(), AppError> {
        let _op = self.lock_op()?;
        self.restore_inner()
    }

    fn was_saved(&self) -> bool {
        crate::util::lock_mutex(&self.saved_content, "ClipboardManager::saved_content")
            .is_some_and(|guard| guard.is_some())
    }
}

impl Default for ClipboardManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Mock clipboard for testing pipeline phases.
/// Interior-mutable so tests can share `Arc<MockClipboard>` with the pipeline
/// (as `Arc<dyn ClipboardProvider>`) and still assert through the concrete handle.
pub struct MockClipboard {
    saved: AtomicBool,
    injected: Mutex<Vec<String>>,
    restored: AtomicBool,
    save_error: Mutex<Option<String>>,
    inject_error: Mutex<Option<String>>,
    restore_error: Mutex<Option<String>>,
    op: Mutex<()>,
}

impl MockClipboard {
    /// Create a new mock clipboard in the initial (no save/inject/restore) state.
    pub fn new() -> Self {
        Self {
            saved: AtomicBool::new(false),
            injected: Mutex::new(Vec::new()),
            restored: AtomicBool::new(false),
            save_error: Mutex::new(None),
            inject_error: Mutex::new(None),
            restore_error: Mutex::new(None),
            op: Mutex::new(()),
        }
    }

    /// `save()` returns `AppError::Clipboard(msg)` instead of succeeding.
    pub fn with_save_error(self, msg: &str) -> Self {
        if let Ok(mut guard) = self.save_error.lock() {
            *guard = Some(msg.to_string());
        }
        self
    }

    /// `inject_text()` returns `AppError::Clipboard(msg)` instead of succeeding.
    pub fn with_inject_error(self, msg: &str) -> Self {
        if let Ok(mut guard) = self.inject_error.lock() {
            *guard = Some(msg.to_string());
        }
        self
    }

    /// `restore()` returns `AppError::Clipboard(msg)` instead of succeeding.
    pub fn with_restore_error(self, msg: &str) -> Self {
        if let Ok(mut guard) = self.restore_error.lock() {
            *guard = Some(msg.to_string());
        }
        self
    }

    pub fn saved(&self) -> bool {
        self.saved.load(Ordering::SeqCst)
    }

    pub fn restored(&self) -> bool {
        self.restored.load(Ordering::SeqCst)
    }

    pub fn injected(&self) -> Vec<String> {
        self.injected.lock().map(|g| g.clone()).unwrap_or_default()
    }

    fn save_inner(&self) -> Result<(), AppError> {
        if let Ok(guard) = self.save_error.lock() {
            if let Some(msg) = guard.as_ref() {
                return Err(AppError::Clipboard(msg.clone()));
            }
        }
        self.saved.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn inject_inner(&self, text: &str) -> Result<(), AppError> {
        if let Ok(guard) = self.inject_error.lock() {
            if let Some(msg) = guard.as_ref() {
                return Err(AppError::Clipboard(msg.clone()));
            }
        }
        if let Some(mut guard) = crate::util::lock_mutex(&self.injected, "MockClipboard::injected")
        {
            guard.push(text.to_string());
        }
        Ok(())
    }

    fn restore_inner(&self) -> Result<(), AppError> {
        if let Ok(guard) = self.restore_error.lock() {
            if let Some(msg) = guard.as_ref() {
                return Err(AppError::Clipboard(msg.clone()));
            }
        }
        self.restored.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Same poison behaviour as production: surfaces an Err instead of panicking.
    fn lock_op(&self) -> Result<std::sync::MutexGuard<'_, ()>, AppError> {
        crate::util::lock_mutex(&self.op, "MockClipboard::op")
            .ok_or_else(|| AppError::Clipboard("op lock poisoned".to_string()))
    }
}

impl Default for MockClipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardProvider for MockClipboard {
    fn save(&self) -> Result<(), AppError> {
        let _op = self.lock_op()?;
        self.save_inner()
    }

    fn inject_text(&self, text: &str) -> Result<(), AppError> {
        let _op = self.lock_op()?;
        self.inject_inner(text)
    }

    fn save_and_inject(&self, text: &str) -> Result<(), AppError> {
        let _op = self.lock_op()?;
        self.save_inner()?;
        self.inject_inner(text)
    }

    fn restore(&self) -> Result<(), AppError> {
        let _op = self.lock_op()?;
        self.restore_inner()
    }

    fn was_saved(&self) -> bool {
        self.saved.load(Ordering::SeqCst)
    }
}

/// Read text from the Windows clipboard using raw Win32 API.
/// Times out after CLIPBOARD_TIMEOUT if another process holds the clipboard.
fn read_clipboard() -> Result<String, AppError> {
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    with_clipboard_timeout(
        move || unsafe {
            // SAFETY: OpenClipboard with None opens the clipboard associated with
            // the current task. We close it before returning (CloseClipboard below).
            // All Win32 clipboard APIs require the clipboard to be open; we open
            // it here and ensure CloseClipboard is called on all exit paths.
            OpenClipboard(None).map_err(|e| AppError::Clipboard(format!("open failed: {e}")))?;

            let result = (|| -> Result<String, AppError> {
                let handle = GetClipboardData(13u32)
                    .map_err(|e| AppError::Clipboard(format!("get data failed: {e}")))?;

                // SAFETY: GlobalLock locks the clipboard data handle returned by
                // GetClipboardData. The handle is valid and we unlock it below.
                let ptr = GlobalLock(windows::Win32::Foundation::HGLOBAL(handle.0));
                if ptr.is_null() {
                    return Err(AppError::Clipboard("lock failed: null pointer".to_string()));
                }

                let block_size = GlobalSize(windows::Win32::Foundation::HGLOBAL(handle.0));
                let len = if block_size > 0 {
                    (block_size / 2).saturating_sub(1) as usize
                } else {
                    let u16_ptr = ptr as *const u16;
                    let mut l = 0usize;
                    while *u16_ptr.add(l) != 0 {
                        l += 1;
                    }
                    l
                };

                let u16_ptr = ptr as *const u16;
                // SAFETY: ptr points to a valid GlobalLock'd memory region of at least
                // `len` u16 elements. We verified len bounds above (block_size or null-term).
                let slice = std::slice::from_raw_parts(u16_ptr, len);
                let text = String::from_utf16(slice)
                    .map_err(|e| AppError::Clipboard(format!("utf16 decode failed: {e}")))?;

                let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(handle.0));
                Ok(text)
            })();

            let _ = CloseClipboard();
            result
        },
        CLIPBOARD_TIMEOUT,
        "read_clipboard",
    )
}

/// Write text to the clipboard, retrying on failure.
fn write_clipboard_with_retry(text: &str) -> Result<(), AppError> {
    for attempt in 0..MAX_RETRIES {
        match write_clipboard(text) {
            Ok(()) => return Ok(()),
            Err(_) if attempt < MAX_RETRIES - 1 => {
                std::thread::sleep(RETRY_DELAY);
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// Write text to the Windows clipboard using raw Win32 API.
/// Times out after CLIPBOARD_TIMEOUT if another process holds the clipboard.
fn write_clipboard(text: &str) -> Result<(), AppError> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

    let text = text.to_string();
    with_clipboard_timeout(
        move || unsafe {
            // SAFETY: OpenClipboard with None opens the clipboard for the current task.
            // We close it before returning (CloseClipboard below).
            OpenClipboard(None).map_err(|e| AppError::Clipboard(format!("open failed: {e}")))?;
            EmptyClipboard().map_err(|e| AppError::Clipboard(format!("empty failed: {e}")))?;

            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0u16)).collect();
            let byte_len = wide.len() * 2;

            let hglobal = GlobalAlloc(GMEM_MOVEABLE, byte_len)
                .map_err(|e| AppError::Clipboard(format!("alloc failed: {e}")))?;

            // SAFETY: GlobalLock on a freshly allocated, valid HGLOBAL. We unlock below.
            let ptr = GlobalLock(hglobal);
            if ptr.is_null() {
                return Err(AppError::Clipboard("lock failed".to_string()));
            }

            // SAFETY: ptr is a valid, locked buffer of byte_len bytes. `wide` has
            // wide.len() u16 elements = byte_len bytes (allocated above). No overlap
            // because ptr is a fresh allocation.
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr as *mut u16, wide.len());
            let _ = GlobalUnlock(hglobal);

            let handle = windows::Win32::Foundation::HANDLE(hglobal.0);
            SetClipboardData(13u32, handle)
                .map_err(|e| AppError::Clipboard(format!("set data failed: {e}")))?;

            let _ = CloseClipboard();
            Ok(())
        },
        CLIPBOARD_TIMEOUT,
        "write_clipboard",
    )
}

/// Simulate Ctrl+V keypress using Win32 SendInput.
fn simulate_paste() -> Result<(), AppError> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_TYPE, KEYEVENTF_KEYUP, SendInput, VK_CONTROL, VK_V,
    };

    // SAFETY: We zero-initialize a 4-element INPUT array, then populate each entry
    // with valid keyboard event data (Ctrl down, V down, V up, Ctrl up). SendInput
    // receives a valid slice and the correct struct size. All fields are properly set.
    unsafe {
        let mut inputs: [INPUT; 4] = std::mem::zeroed();

        inputs[0].r#type = INPUT_TYPE(1);
        inputs[0].Anonymous.ki.wVk = VK_CONTROL;

        inputs[1].r#type = INPUT_TYPE(1);
        inputs[1].Anonymous.ki.wVk = VK_V;

        inputs[2].r#type = INPUT_TYPE(1);
        inputs[2].Anonymous.ki.wVk = VK_V;
        inputs[2].Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;

        inputs[3].r#type = INPUT_TYPE(1);
        inputs[3].Anonymous.ki.wVk = VK_CONTROL;
        inputs[3].Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;

        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_clipboard_timeout_success() {
        let result =
            with_clipboard_timeout(|| Ok::<_, AppError>(42), Duration::from_secs(5), "test");
        match result {
            Ok(v) => assert_eq!(v, 42),
            Err(e) => panic!("expected Ok, got Err: {e}"),
        }
    }

    #[test]
    fn test_with_clipboard_timeout_times_out() {
        let result = with_clipboard_timeout(
            || {
                thread::sleep(Duration::from_secs(10));
                Ok::<_, AppError>(42)
            },
            Duration::from_millis(100),
            "test_op",
        );
        let err = match result {
            Ok(v) => panic!("expected Err, got Ok({v})"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("timed out"),
            "error should mention timeout: {err}"
        );
        assert!(
            err.contains("test_op"),
            "error should mention operation name: {err}"
        );
    }

    #[test]
    fn test_clipboard_manager_save() {
        let manager = ClipboardManager::new();
        assert!(manager.save().is_ok());
    }

    #[test]
    fn test_mock_clipboard() {
        let mock = MockClipboard::new();
        assert!(mock.save().is_ok());
        assert!(mock.saved());
        assert!(mock.inject_text("hello").is_ok());
        assert_eq!(mock.injected(), vec!["hello"]);
        assert!(mock.restore().is_ok());
        assert!(mock.restored());
    }

    #[test]
    fn test_mock_clipboard_save_and_inject() {
        let mock = MockClipboard::new();
        assert!(mock.save_and_inject("hi").is_ok());
        assert!(mock.saved());
        assert_eq!(mock.injected(), vec!["hi"]);
    }

    #[test]
    fn test_mock_clipboard_error_injection() {
        let mock = MockClipboard::new().with_inject_error("boom");
        assert!(mock.inject_text("x").is_err());
        assert!(mock.injected().is_empty());
    }
}
