use crate::error::CommandError;
use crate::perf::{PerfHistory, PerfMetrics};
use std::sync::Arc;

/// Return recent performance metrics history.
#[tauri::command]
pub fn get_perf_history(
    perf: tauri::State<'_, Arc<PerfHistory>>,
    n: Option<usize>,
) -> Result<Vec<PerfMetrics>, CommandError> {
    Ok(perf.recent(n.unwrap_or(10)))
}
