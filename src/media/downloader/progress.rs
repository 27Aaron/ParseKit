//! Fixed-threshold download progress reporting.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::{DownloadProgress, ProgressCallback};

const PROGRESS_THRESHOLDS: [u8; 5] = [20, 40, 60, 80, 100];

pub(super) struct ProgressReporter {
    callback: ProgressCallback,
    total_bytes: u64,
    next_threshold: usize,
    active: Arc<AtomicBool>,
}

impl ProgressReporter {
    pub(super) fn new(
        content_length: Option<u64>,
        callback: Option<ProgressCallback>,
    ) -> (Option<Self>, Option<ProgressGuard>) {
        let Some(total_bytes) = content_length.filter(|total| *total > 0) else {
            return (None, None);
        };
        let Some(callback) = callback else {
            return (None, None);
        };

        let active = Arc::new(AtomicBool::new(true));
        (
            Some(Self {
                callback,
                total_bytes,
                next_threshold: 0,
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
        while let Some(&percent) = PROGRESS_THRESHOLDS.get(self.next_threshold) {
            if percent == 100 && !include_complete {
                break;
            }
            if u128::from(downloaded_bytes) * 100
                < u128::from(self.total_bytes) * u128::from(percent)
            {
                break;
            }

            self.next_threshold += 1;
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
