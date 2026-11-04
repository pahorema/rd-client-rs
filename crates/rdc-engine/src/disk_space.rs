//! Queries free disk space and drives low-space pause/resume hysteresis.

use std::{
    fmt::Display,
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// The most recent pause-related status sent to connected clients.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskSpaceStatus {
    /// Whether internal downloads are paused because disk space is low.
    pub is_paused: bool,
    /// Available whole gibibytes at the last relevant check, or -1 when unknown.
    #[serde(rename = "availableSpaceGB")]
    pub available_space_gb: i64,
    /// The configured pause threshold, in gibibytes.
    #[serde(rename = "thresholdGB")]
    pub threshold_gb: i32,
    /// Time at which the status was refreshed.
    pub last_check_time: Timestamp,
}

/// An effect for the engine to apply; pause/resume must target internal downloads only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiskSpaceAction {
    /// Pause active internal downloads and prevent new internal downloads from starting.
    Pause,
    /// Resume active internal downloads and permit new internal downloads to start.
    Resume,
    /// Store and broadcast the latest status snapshot.
    StatusChanged(DiskSpaceStatus),
}

/// Pure state machine with a resume threshold twice the configured pause threshold.
#[derive(Clone, Debug, Default)]
pub struct DiskSpaceMonitor {
    paused: bool,
    last_status: Option<DiskSpaceStatus>,
}

impl DiskSpaceMonitor {
    /// Whether the monitor currently requires internal downloads to remain paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Returns no status until the first pause-related event.
    pub fn last_status(&self) -> Option<&DiskSpaceStatus> {
        self.last_status.as_ref()
    }

    /// Evaluates a measurement and advances state without performing any external actions.
    ///
    /// An unknown measurement leaves state unchanged, except disabling the threshold
    /// always resumes. Zero available space is a valid measurement and pauses downloads.
    /// Already-paused low-space checks refresh status; the hysteresis band stays silent.
    pub fn check(
        &mut self,
        threshold_gb: i32,
        available_space_gb: Option<i64>,
        now: Timestamp,
    ) -> Vec<DiskSpaceAction> {
        let mut actions = Vec::new();
        let disabled = threshold_gb <= 0;
        let should_pause =
            !disabled && available_space_gb.is_some_and(|space| space < i64::from(threshold_gb));
        let should_resume = disabled
            || available_space_gb.is_some_and(|space| space >= i64::from(threshold_gb) * 2);
        if should_pause {
            if !self.paused {
                self.paused = true;
                actions.push(DiskSpaceAction::Pause);
            }
        } else if should_resume && self.paused {
            self.paused = false;
            actions.push(DiskSpaceAction::Resume);
        } else {
            return actions;
        }
        let status = DiskSpaceStatus {
            is_paused: self.paused,
            available_space_gb: available_space_gb.unwrap_or(-1),
            threshold_gb,
            last_check_time: now,
        };
        self.last_status = Some(status.clone());
        actions.push(DiskSpaceAction::StatusChanged(status));
        actions
    }
}

/// Measures available whole gibibytes, returning `None` for a missing directory or I/O error.
pub fn free_space_gb(path: &Path) -> Option<i64> {
    if !path.is_dir() {
        return None;
    }
    fs4::available_space(path)
        .ok()
        .and_then(|bytes| i64::try_from(bytes / (1024 * 1024 * 1024)).ok())
}

/// Settings read on each monitor pass; updates interrupt the current sleep.
#[derive(Clone, Debug, Default)]
pub struct DiskSpaceConfig {
    /// Local download directory whose containing filesystem is measured.
    pub path: PathBuf,
    /// Nonpositive values disable the monitor and resume paused downloads.
    pub threshold_gb: i32,
    /// Minutes between checks, clamped to at least one.
    pub interval_minutes: i32,
}

/// Runs immediate-first monitoring until cancellation, applying actions in order.
///
/// The callback must make pause/resume idempotent. Failed actions are logged and retried
/// after the normal interval, not in a tight loop. State advances only after all actions
/// succeed. Disabled configuration and unreadable paths are rechecked after one minute.
/// Settings-channel closure retains its final value. Cancellation aborts in-flight actions.
pub async fn run<F, Fut, E>(
    mut config: watch::Receiver<DiskSpaceConfig>,
    cancel: CancellationToken,
    mut apply: F,
) where
    F: FnMut(DiskSpaceAction) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Display,
{
    let mut monitor = DiskSpaceMonitor::default();
    let mut config_open = true;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let settings = config.borrow_and_update().clone();
        let available = if settings.threshold_gb > 0 {
            let path = settings.path.clone();
            tokio::select! {
                () = cancel.cancelled() => return,
                result = tokio::task::spawn_blocking(move || free_space_gb(&path)) => result.ok().flatten(),
            }
        } else {
            None
        };
        if settings.threshold_gb > 0 && available.is_none() {
            tracing::warn!(path = %settings.path.display(), "Unable to query download directory disk space");
        }
        let mut proposed = monitor.clone();
        let actions = proposed.check(settings.threshold_gb, available, Timestamp::now());
        let mut succeeded = true;
        for action in actions {
            let result = tokio::select! {
                () = cancel.cancelled() => return,
                result = apply(action) => result,
            };
            if let Err(error) = result {
                tracing::error!(%error, "Unable to apply disk space action");
                succeeded = false;
                break;
            }
        }
        if succeeded {
            monitor = proposed;
        }
        let minutes = if settings.threshold_gb <= 0 || available.is_none() {
            1
        } else {
            settings.interval_minutes.max(1)
        };
        let delay = Duration::from_secs(u64::try_from(minutes).unwrap_or(1) * 60);
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(delay) => {},
            changed = config.changed(), if config_open => {
                if changed.is_err() {
                    config_open = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pauses_refreshes_and_resumes_at_double_threshold() {
        let mut monitor = DiskSpaceMonitor::default();
        let now = Timestamp::UNIX_EPOCH;
        assert_eq!(monitor.check(10, Some(10), now), []);
        assert!(monitor.last_status().is_none());
        let actions = monitor.check(10, Some(9), now);
        assert!(matches!(
            actions.as_slice(),
            [DiskSpaceAction::Pause, DiskSpaceAction::StatusChanged(_)]
        ));
        assert!(monitor.is_paused());
        assert!(matches!(
            monitor.check(10, Some(8), now).as_slice(),
            [DiskSpaceAction::StatusChanged(_)]
        ));
        assert_eq!(monitor.check(10, Some(19), now), []);
        assert!(monitor.is_paused());
        assert!(matches!(
            monitor.check(10, Some(20), now).as_slice(),
            [DiskSpaceAction::Resume, DiskSpaceAction::StatusChanged(_)]
        ));
        assert!(!monitor.is_paused());
    }

    #[test]
    fn zero_space_pauses_unknown_does_not_and_disabling_resumes() {
        let mut monitor = DiskSpaceMonitor::default();
        let now = Timestamp::UNIX_EPOCH;
        assert_eq!(monitor.check(10, None, now), []);
        assert_eq!(monitor.check(10, Some(0), now)[0], DiskSpaceAction::Pause);
        assert_eq!(monitor.check(10, None, now), []);
        assert_eq!(monitor.check(0, None, now)[0], DiskSpaceAction::Resume);
        assert_eq!(monitor.last_status().unwrap().available_space_gb, -1);
        assert_eq!(monitor.check(0, None, now), []);
    }

    #[test]
    fn threshold_math_does_not_overflow_and_configuration_changes_apply() {
        let mut monitor = DiskSpaceMonitor::default();
        let now = Timestamp::UNIX_EPOCH;
        monitor.check(i32::MAX, Some(1), now);
        assert_eq!(
            monitor.check(i32::MAX, Some(i64::from(i32::MAX) * 2 - 1), now),
            []
        );
        assert_eq!(monitor.check(1, Some(2), now)[0], DiskSpaceAction::Resume);
    }

    #[test]
    fn serializes_required_camel_case_keys() {
        let status = DiskSpaceStatus {
            is_paused: true,
            available_space_gb: 2,
            threshold_gb: 5,
            last_check_time: Timestamp::UNIX_EPOCH,
        };
        let value = serde_json::to_value(&status).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"isPaused":true,"availableSpaceGB":2,"thresholdGB":5,"lastCheckTime":"1970-01-01T00:00:00Z"})
        );
        assert_eq!(
            serde_json::from_value::<DiskSpaceStatus>(value).unwrap(),
            status
        );
    }

    #[test]
    fn queries_existing_directory_and_ignores_missing_paths_and_files() {
        let temp = tempfile::tempdir().unwrap();
        assert!(free_space_gb(temp.path()).is_some());
        assert_eq!(free_space_gb(&temp.path().join("missing")), None);
        let file = temp.path().join("file");
        std::fs::write(&file, "data").unwrap();
        assert_eq!(free_space_gb(&file), None);
    }

    #[tokio::test]
    async fn cancellation_stops_loop_with_closed_settings_channel() {
        let (sender, config) = watch::channel(DiskSpaceConfig::default());
        drop(sender);
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let task = tokio::spawn(run(config, cancel, |_| async {
            Ok::<_, std::io::Error>(())
        }));
        tokio::task::yield_now().await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }
}
