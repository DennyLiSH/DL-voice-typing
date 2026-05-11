use std::sync::Arc;
use tauri::{Emitter, Manager, Position};

/// Abstract window operations for the hotkey pipeline.
/// Decouples pipeline logic from Tauri-specific window names and positioning.
pub trait WindowController: Send + Sync {
    /// Show the floating recording indicator near the text caret.
    /// Returns `true` if the window was found and shown.
    fn show_floating_near_caret(&self) -> bool;
    /// Hide the floating recording indicator.
    fn hide_floating(&self);
    /// Show the review window near the text caret, clamped to monitor bounds.
    /// Returns `true` if the window was found and shown.
    fn show_review_near_caret(&self) -> bool;
    /// Hide the review window.
    fn hide_review(&self);
    /// Focus the review window.
    /// Returns `true` if the window was found.
    fn focus_review(&self) -> bool;
    /// Execute JavaScript inside the review window.
    /// Returns `true` if the window was found.
    fn eval_review_js(&self, js: &str) -> bool;
    /// Emit a review-show event.
    fn emit_review_show(&self);
    /// Emit a review-final-text event.
    fn emit_review_final_text(&self, text: &str);
}

/// Tauri-based implementation of window operations.
pub struct TauriWindowController {
    app: tauri::AppHandle,
}

impl TauriWindowController {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl WindowController for TauriWindowController {
    fn show_floating_near_caret(&self) -> bool {
        let (cx, cy) = crate::win32::get_caret_screen_pos();
        let win_half = 90.0;
        let offset = 40.0;
        let x = cx - offset - win_half;
        let y = cy - offset - win_half;
        if let Some(win) = self.app.get_webview_window("floating") {
            let _ = win.set_position(Position::Logical(tauri::LogicalPosition::new(x, y)));
            let _ = win.show();
            true
        } else {
            false
        }
    }

    fn hide_floating(&self) {
        if let Some(win) = self.app.get_webview_window("floating") {
            let _ = win.hide();
        }
    }

    fn show_review_near_caret(&self) -> bool {
        let (cx, cy) = crate::win32::get_caret_screen_pos();
        let mut x = cx + 10.0;
        let mut y = cy + 20.0;
        let win_w = 420.0_f64;
        let win_h = 220.0_f64;
        if let Some((left, top, right, bottom)) =
            crate::win32::get_monitor_work_area(cx as i32, cy as i32)
        {
            x = x.min(right as f64 - win_w).max(left as f64);
            y = y.min(bottom as f64 - win_h).max(top as f64);
        }
        if let Some(win) = self.app.get_webview_window("review") {
            let _ = win.set_position(Position::Logical(tauri::LogicalPosition::new(x, y)));
            let _ = win.show();
            let _ = self.app.emit("review-show", ());
            let _ = win.set_focus();
            true
        } else {
            false
        }
    }

    fn hide_review(&self) {
        if let Some(win) = self.app.get_webview_window("review") {
            let _ = win.hide();
        }
    }

    fn focus_review(&self) -> bool {
        if let Some(win) = self.app.get_webview_window("review") {
            let _ = win.set_focus();
            true
        } else {
            false
        }
    }

    fn eval_review_js(&self, js: &str) -> bool {
        if let Some(win) = self.app.get_webview_window("review") {
            let _ = win.eval(js);
            true
        } else {
            false
        }
    }

    fn emit_review_show(&self) {
        let _ = self.app.emit("review-show", ());
    }

    fn emit_review_final_text(&self, text: &str) {
        let _ = self.app.emit("review-final-text", text);
    }
}

/// No-op window controller for tests or headless environments.
pub struct NoopWindowController;

impl WindowController for NoopWindowController {
    fn show_floating_near_caret(&self) -> bool {
        true
    }
    fn hide_floating(&self) {}
    fn show_review_near_caret(&self) -> bool {
        true
    }
    fn hide_review(&self) {}
    fn focus_review(&self) -> bool {
        true
    }
    fn eval_review_js(&self, _js: &str) -> bool {
        true
    }
    fn emit_review_show(&self) {}
    fn emit_review_final_text(&self, _text: &str) {}
}

/// Helper to create the appropriate window controller from an AppHandle.
pub fn window_controller_from_app(app: &tauri::AppHandle) -> Arc<dyn WindowController> {
    Arc::new(TauriWindowController::new(app.clone()))
}
