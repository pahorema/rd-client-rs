//! Download Station tasks (`SYNO.DownloadStation2.Task`).

use std::fmt;

use serde::{Deserialize, Deserializer};

use crate::wire::number;

/// State of a Download Station task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TaskStatus {
    /// No state reported.
    #[default]
    None,
    /// Queued.
    Waiting,
    /// Downloading.
    Downloading,
    /// Paused.
    Paused,
    /// Moving the downloaded data to the destination.
    Finishing,
    /// Done.
    Finished,
    /// Verifying downloaded data.
    HashChecking,
    /// About to seed.
    PreSeeding,
    /// Seeding (the download itself is complete).
    Seeding,
    /// Waiting for a file hosting service.
    FileHostingWaiting,
    /// Extracting archives.
    Extracting,
    /// Preprocessing.
    Preprocessing,
    /// Preprocessing passed.
    PreprocessPass,
    /// Downloaded.
    Downloaded,
    /// Postprocessing.
    Postprocessing,
    /// Stalled until a captcha is solved in Download Station.
    CaptchaNeeded,
    /// A code this client does not know.
    Unknown(i64),
}

impl TaskStatus {
    /// The numeric code DSM uses for this state.
    pub fn code(self) -> i64 {
        match self {
            Self::None => 0,
            Self::Waiting => 1,
            Self::Downloading => 2,
            Self::Paused => 3,
            Self::Finishing => 4,
            Self::Finished => 5,
            Self::HashChecking => 6,
            Self::PreSeeding => 7,
            Self::Seeding => 8,
            Self::FileHostingWaiting => 9,
            Self::Extracting => 10,
            Self::Preprocessing => 11,
            Self::PreprocessPass => 12,
            Self::Downloaded => 13,
            Self::Postprocessing => 14,
            Self::CaptchaNeeded => 15,
            Self::Unknown(code) => code,
        }
    }
}

impl From<i64> for TaskStatus {
    fn from(code: i64) -> Self {
        match code {
            0 => Self::None,
            1 => Self::Waiting,
            2 => Self::Downloading,
            3 => Self::Paused,
            4 => Self::Finishing,
            5 => Self::Finished,
            6 => Self::HashChecking,
            7 => Self::PreSeeding,
            8 => Self::Seeding,
            9 => Self::FileHostingWaiting,
            10 => Self::Extracting,
            11 => Self::Preprocessing,
            12 => Self::PreprocessPass,
            13 => Self::Downloaded,
            14 => Self::Postprocessing,
            15 => Self::CaptchaNeeded,
            other => Self::Unknown(other),
        }
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::None => "None",
            Self::Waiting => "Waiting",
            Self::Downloading => "Downloading",
            Self::Paused => "Paused",
            Self::Finishing => "Finishing",
            Self::Finished => "Finished",
            Self::HashChecking => "HashChecking",
            Self::PreSeeding => "PreSeeding",
            Self::Seeding => "Seeding",
            Self::FileHostingWaiting => "FileHostingWaiting",
            Self::Extracting => "Extracting",
            Self::Preprocessing => "Preprocessing",
            Self::PreprocessPass => "PreprocessPass",
            Self::Downloaded => "Downloaded",
            Self::Postprocessing => "Postprocessing",
            Self::CaptchaNeeded => "CaptchaNeeded",
            Self::Unknown(code) => return write!(f, "Unknown({code})"),
        };
        f.write_str(name)
    }
}

impl<'de> Deserialize<'de> for TaskStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = number(deserializer)?;
        Ok(i64::try_from(code).map_or(Self::Unknown(i64::MAX), Self::from))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_round_trip() {
        for code in 0..=15 {
            let status = TaskStatus::from(code);

            assert!(!matches!(status, TaskStatus::Unknown(_)), "{code}");
            assert_eq!(status.code(), code);
        }
        assert_eq!(TaskStatus::from(99), TaskStatus::Unknown(99));
    }

    #[test]
    fn status_displays_its_name() {
        assert_eq!(TaskStatus::CaptchaNeeded.to_string(), "CaptchaNeeded");
        assert_eq!(TaskStatus::Unknown(42).to_string(), "Unknown(42)");
    }

    #[test]
    fn status_parses_numbers_and_numeric_strings() {
        let parsed: Vec<TaskStatus> = serde_json::from_str(r#"[8, "13"]"#).unwrap();

        assert_eq!(parsed, [TaskStatus::Seeding, TaskStatus::Downloaded]);
    }
}
