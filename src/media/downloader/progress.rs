//! Percent-step download progress reporting.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::{DownloadProgress, ProgressCallback};

/// Reports each whole percentage when the total length is known.
pub(super) struct ProgressReporter {
    callback: ProgressCallback,
    total_bytes: u64,
    next_percent: u16,
    active: Arc<AtomicBool>,
}

impl ProgressReporter {
    pub(super) fn new(
        content_length: Option<u64>,
        callback: Option<ProgressCallback>,
        initial_bytes: u64,
    ) -> (Option<Self>, Option<ProgressGuard>) {
        let Some(total_bytes) = content_length.filter(|total| *total > 0) else {
            return (None, None);
        };
        let Some(callback) = callback else {
            return (None, None);
        };

        let active = Arc::new(AtomicBool::new(true));
        let already_reached =
            ((u128::from(initial_bytes) * 100) / u128::from(total_bytes)).min(99) as u16;
        (
            Some(Self {
                callback,
                total_bytes,
                // Resume after thresholds already represented on disk.
                next_percent: already_reached.saturating_add(1),
                active: Arc::clone(&active),
            }),
            Some(ProgressGuard { active }),
        )
    }

    pub(super) fn report_intermediate(&mut self, downloaded_bytes: u64) {
        self.report_crossed(downloaded_bytes, false);
    }

    pub(super) fn report_complete(&mut self, downloaded_bytes: u64) {
        self.report_crossed(downloaded_bytes, true);
    }

    fn report_crossed(&mut self, downloaded_bytes: u64, include_complete: bool) {
        let reached =
            ((u128::from(downloaded_bytes) * 100) / u128::from(self.total_bytes)).min(100) as u16;
        let max_emit = if include_complete {
            100
        } else {
            reached.min(99)
        };

        while self.next_percent <= max_emit {
            let percent =
                u8::try_from(self.next_percent).expect("progress percentages never exceed 100");
            self.next_percent += 1;
            if self.active.load(Ordering::Acquire) {
                (self.callback)(DownloadProgress {
                    downloaded_bytes,
                    total_bytes: self.total_bytes,
                    percent,
                });
            }
        }
    }
}

pub(super) struct ProgressGuard {
    active: Arc<AtomicBool>,
}

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}
