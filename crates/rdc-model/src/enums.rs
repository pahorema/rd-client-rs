//! Enumerations persisted as integers in the database and exchanged as
//! integers over the HTTP API.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// Error returned when an integer does not correspond to any variant.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{value} is not a valid {type_name} value")]
pub struct UnknownValueError {
    type_name: &'static str,
    value: i64,
}

impl UnknownValueError {
    /// Name of the enumeration that rejected the value.
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// The rejected integer.
    pub fn value(&self) -> i64 {
        self.value
    }
}

/// Error returned when a text is neither a variant name nor a valid number.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{input:?} is not a valid {type_name} value")]
pub struct ParseEnumError {
    type_name: &'static str,
    input: String,
}

impl ParseEnumError {
    /// Name of the enumeration that rejected the text.
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// The rejected text.
    pub fn input(&self) -> &str {
        &self.input
    }
}

/// Parses the integer form of an enumeration value the way stored settings
/// rows write it: optional surrounding whitespace and an optional sign.
fn parse_number(text: &str) -> Option<i64> {
    let first = text.chars().next()?;
    if first.is_ascii_digit() || first == '+' || first == '-' {
        text.parse().ok()
    } else {
        None
    }
}

/// Defines an enumeration with stable numeric values.
///
/// Every generated type serializes as its integer, converts from `i64`,
/// parses from its numeric text or variant name (case-sensitive, plus any
/// listed legacy names), displays its canonical name and offers a UI label.
/// The first listed variant is the `Default`, matching the value an absent
/// or NULL column decodes to.
macro_rules! numeric_enum {
    (
        $(#[$meta:meta])*
        $name:ident {
            $(#[$first_meta:meta])*
            $first:ident = $first_value:literal => $first_label:literal,
            $(
                $(#[$variant_meta:meta])*
                $variant:ident = $value:literal => $label:literal,
            )*
        }
        $(legacy { $($legacy:literal => $legacy_variant:ident),+ $(,)? })?
    ) => {
        $(#[$meta])*
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            PartialOrd,
            Ord,
            Default,
            serde_repr::Serialize_repr,
            serde_repr::Deserialize_repr,
        )]
        #[repr(i32)]
        pub enum $name {
            $(#[$first_meta])*
            #[default]
            $first = $first_value,
            $(
                $(#[$variant_meta])*
                $variant = $value,
            )*
        }

        impl $name {
            /// Every variant in ascending numeric order.
            pub const ALL: &'static [Self] = &[Self::$first, $(Self::$variant),*];

            /// `(value, label)` pairs in ascending numeric order, as offered
            /// by selection lists.
            pub const OPTIONS: &'static [(i32, &'static str)] =
                &[($first_value, $first_label), $(($value, $label)),*];

            /// The stable integer stored in the database.
            pub const fn as_i32(self) -> i32 {
                self as i32
            }

            /// The canonical variant name.
            pub const fn name(self) -> &'static str {
                match self {
                    Self::$first => stringify!($first),
                    $(Self::$variant => stringify!($variant),)*
                }
            }

            /// The human-readable label shown in the web UI.
            pub const fn label(self) -> &'static str {
                match self {
                    Self::$first => $first_label,
                    $(Self::$variant => $label,)*
                }
            }

            fn from_name(name: &str) -> Option<Self> {
                match name {
                    stringify!($first) => Some(Self::$first),
                    $(stringify!($variant) => Some(Self::$variant),)*
                    $($($legacy => Some(Self::$legacy_variant),)+)?
                    _ => None,
                }
            }
        }

        impl TryFrom<i64> for $name {
            type Error = UnknownValueError;

            fn try_from(value: i64) -> Result<Self, UnknownValueError> {
                match value {
                    $first_value => Ok(Self::$first),
                    $($value => Ok(Self::$variant),)*
                    _ => Err(UnknownValueError {
                        type_name: stringify!($name),
                        value,
                    }),
                }
            }
        }

        impl From<$name> for i32 {
            fn from(value: $name) -> Self {
                value.as_i32()
            }
        }

        impl From<$name> for i64 {
            fn from(value: $name) -> Self {
                i64::from(value.as_i32())
            }
        }

        impl FromStr for $name {
            type Err = ParseEnumError;

            /// Accepts the numeric value or the variant name (legacy names
            /// included); surrounding whitespace is ignored, names are
            /// case-sensitive.
            fn from_str(text: &str) -> Result<Self, Self::Err> {
                let trimmed = text.trim();
                let parsed = match parse_number(trimmed) {
                    Some(number) => Self::try_from(number).ok(),
                    None => Self::from_name(trimmed),
                };
                parsed.ok_or_else(|| ParseEnumError {
                    type_name: stringify!($name),
                    input: text.to_owned(),
                })
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}

numeric_enum! {
    /// Torrent state as reported by the debrid provider (`Torrents.RdStatus`).
    TorrentStatus {
        /// Waiting in the local queue; not sent to the provider yet.
        Queued = 0 => "Queued",
        /// The provider is converting or checking the torrent.
        Processing = 1 => "Processing",
        /// The provider waits for the files to download to be selected.
        WaitingForFileSelection = 2 => "WaitingForFileSelection",
        /// The provider is downloading the torrent.
        Downloading = 3 => "Downloading",
        /// The provider finished; download links are available.
        Finished = 4 => "Finished",
        /// The provider is uploading or compressing the result.
        Uploading = 5 => "Uploading",
        /// The provider reported an error.
        Error = 99 => "Error",
    }
}

numeric_enum! {
    /// Debrid service a torrent is managed on.
    Provider {
        /// Real-Debrid.
        RealDebrid = 0 => "RealDebrid",
        /// The alldebrid.com service.
        AllDebrid = 1 => "AllDebrid",
        /// Premiumize.
        Premiumize = 2 => "Premiumize",
        /// The torbox.app service.
        TorBox = 3 => "TorBox",
        /// Debrid-Link.
        DebridLink = 4 => "DebridLink",
        /// HIGHWAY.
        Highway = 5 => "HIGHWAY",
    }
    // Legacy stored name.
    legacy { "HIGHWAY" => Highway }
}

numeric_enum! {
    /// Engine that transfers finished files from the provider to the host.
    DownloadClient {
        /// The built-in multi-connection HTTP downloader.
        Internal = 0 => "Internal downloader",
        /// An external aria2 instance driven over JSON-RPC.
        Aria2c = 1 => "Aria2c",
        /// Symbolic links into an rclone mount of the provider.
        Symlink = 2 => "Symlink Downloader",
        /// A Synology Download Station.
        DownloadStation = 3 => "Synology DownloadStation",
    }
    // Legacy stored name.
    legacy { "Bezzad" => Internal }
}

numeric_enum! {
    /// Which files of a torrent are selected on the provider.
    TorrentDownloadAction {
        /// Every file of the torrent.
        DownloadAll = 0 => "Download All Files",
        /// Every file the provider already has available.
        DownloadAvailableFiles = 1 => "Download All Available Files",
        /// Only the files listed in `download_manual_files`.
        DownloadManual = 2 => "Manually Select Files",
    }
}

numeric_enum! {
    /// What to remove once every file reached the host.
    TorrentFinishedAction {
        /// Keep the torrent everywhere.
        None = 0 => "No Action",
        /// Remove the torrent from the provider and from rd-client.
        RemoveAllTorrents = 1 => "Remove Torrent From Client And Provider",
        /// Remove the torrent from the provider only.
        RemoveProvider = 2 => "Remove Torrent From Provider",
        /// Remove the torrent from rd-client only.
        RemoveClient = 3 => "Remove Torrent From Client",
    }
    // Legacy stored name.
    legacy { "RemoveRealDebrid" => RemoveProvider }
}

numeric_enum! {
    /// Whether finished provider files are transferred to the host at all.
    TorrentHostDownloadAction {
        /// Download every selected file to the host.
        DownloadAll = 0 => "Download all files to host",
        /// Keep the files on the provider only.
        DownloadNone = 1 => "Don't download any files to host",
    }
}

numeric_enum! {
    /// Verbosity of the application log.
    LogLevel {
        /// Everything, including tracing of internals.
        Verbose = 0 => "Verbose",
        /// Diagnostic messages.
        Debug = 1 => "Debug",
        /// Normal operational messages.
        Information = 2 => "Information",
        /// Unexpected but recoverable conditions.
        Warning = 3 => "Warning",
        /// Failures only.
        Error = 4 => "Error",
    }
}

numeric_enum! {
    /// Verbosity of the download client log; `None` disables it.
    DownloadClientLogLevel {
        /// Everything, including tracing of internals.
        Verbose = 0 => "Verbose",
        /// Diagnostic messages.
        Debug = 1 => "Debug",
        /// Normal operational messages.
        Information = 2 => "Information",
        /// Unexpected but recoverable conditions.
        Warning = 3 => "Warning",
        /// Failures only.
        Error = 4 => "Error",
        /// No download client logging.
        None = 5 => "None",
    }
}

numeric_enum! {
    /// How users authenticate with the web UI and the APIs.
    AuthenticationType {
        /// A user name and password login.
        UserNamePassword = 0 => "Username + Password",
        /// No authentication: anyone reaching the URL has full access.
        None = 1 => "No Authentication",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_stable() {
        let values: Vec<i32> = TorrentStatus::ALL.iter().map(|s| s.as_i32()).collect();
        assert_eq!(values, [0, 1, 2, 3, 4, 5, 99]);
        let values: Vec<i32> = Provider::ALL.iter().map(|p| p.as_i32()).collect();
        assert_eq!(values, [0, 1, 2, 3, 4, 5]);
        let values: Vec<i32> = DownloadClient::ALL.iter().map(|c| c.as_i32()).collect();
        assert_eq!(values, [0, 1, 2, 3]);
        let values: Vec<i32> = TorrentDownloadAction::ALL
            .iter()
            .map(|a| a.as_i32())
            .collect();
        assert_eq!(values, [0, 1, 2]);
        let values: Vec<i32> = TorrentFinishedAction::ALL
            .iter()
            .map(|a| a.as_i32())
            .collect();
        assert_eq!(values, [0, 1, 2, 3]);
        let values: Vec<i32> = TorrentHostDownloadAction::ALL
            .iter()
            .map(|a| a.as_i32())
            .collect();
        assert_eq!(values, [0, 1]);
    }

    #[test]
    fn log_levels_and_authentication_round_trip() {
        assert_eq!(LogLevel::default(), LogLevel::Verbose);
        assert_eq!("Error".parse(), Ok(LogLevel::Error));
        assert_eq!("4".parse(), Ok(LogLevel::Error));
        assert!("None".parse::<LogLevel>().is_err());
        assert_eq!("None".parse(), Ok(DownloadClientLogLevel::None));
        assert_eq!(DownloadClientLogLevel::None.as_i32(), 5);
        assert_eq!(DownloadClientLogLevel::Information.label(), "Information");
        assert_eq!("1".parse(), Ok(AuthenticationType::None));
        assert_eq!(
            AuthenticationType::UserNamePassword.label(),
            "Username + Password"
        );
        assert_eq!(AuthenticationType::None.label(), "No Authentication");
    }

    #[test]
    fn converts_from_integers() {
        assert_eq!(TorrentStatus::try_from(99), Ok(TorrentStatus::Error));
        assert_eq!(Provider::try_from(3), Ok(Provider::TorBox));
        let error = TorrentStatus::try_from(6).unwrap_err();
        assert_eq!(error.type_name(), "TorrentStatus");
        assert_eq!(error.value(), 6);
        assert!(Provider::try_from(-1).is_err());
    }

    #[test]
    fn parses_numbers_and_names() {
        assert_eq!("4".parse(), Ok(TorrentStatus::Finished));
        assert_eq!(" 99 ".parse(), Ok(TorrentStatus::Error));
        assert_eq!("+1".parse(), Ok(TorrentStatus::Processing));
        assert_eq!(
            "WaitingForFileSelection".parse(),
            Ok(TorrentStatus::WaitingForFileSelection)
        );
        assert_eq!("DebridLink".parse(), Ok(Provider::DebridLink));
        assert_eq!("Highway".parse(), Ok(Provider::Highway));
        assert_eq!("Internal".parse(), Ok(DownloadClient::Internal));
        assert_eq!(
            "DownloadStation".parse(),
            Ok(DownloadClient::DownloadStation)
        );
        assert_eq!(
            "DownloadManual".parse(),
            Ok(TorrentDownloadAction::DownloadManual)
        );
        assert_eq!("None".parse(), Ok(TorrentFinishedAction::None));
        assert_eq!(
            "RemoveProvider".parse(),
            Ok(TorrentFinishedAction::RemoveProvider)
        );
        assert_eq!(
            "DownloadNone".parse(),
            Ok(TorrentHostDownloadAction::DownloadNone)
        );
    }

    #[test]
    fn parses_legacy_names() {
        assert_eq!("HIGHWAY".parse(), Ok(Provider::Highway));
        assert_eq!("Bezzad".parse(), Ok(DownloadClient::Internal));
        assert_eq!(
            "RemoveRealDebrid".parse(),
            Ok(TorrentFinishedAction::RemoveProvider)
        );
    }

    #[test]
    fn rejects_unknown_text() {
        let error = "realdebrid".parse::<Provider>().unwrap_err();
        assert_eq!(error.type_name(), "Provider");
        assert_eq!(error.input(), "realdebrid");
        assert!("6".parse::<Provider>().is_err());
        assert!("".parse::<Provider>().is_err());
        assert!("1.5".parse::<Provider>().is_err());
    }

    #[test]
    fn displays_canonical_name() {
        assert_eq!(Provider::Highway.to_string(), "Highway");
        assert_eq!(TorrentStatus::Queued.to_string(), "Queued");
        assert_eq!(DownloadClient::Internal.to_string(), "Internal");
        assert_eq!(
            TorrentFinishedAction::RemoveProvider.to_string(),
            "RemoveProvider"
        );
    }

    #[test]
    fn labels_match_the_ui() {
        assert_eq!(Provider::Highway.label(), "HIGHWAY");
        assert_eq!(Provider::RealDebrid.label(), "RealDebrid");
        assert_eq!(TorrentStatus::Error.label(), "Error");
        assert_eq!(Provider::OPTIONS[5], (5, "HIGHWAY"));
        assert_eq!(DownloadClient::Internal.label(), "Internal downloader");
        assert_eq!(
            DownloadClient::DownloadStation.label(),
            "Synology DownloadStation"
        );
        assert_eq!(
            TorrentFinishedAction::RemoveProvider.label(),
            "Remove Torrent From Provider"
        );
        assert_eq!(
            TorrentHostDownloadAction::DownloadNone.label(),
            "Don't download any files to host"
        );
        assert_eq!(
            TorrentDownloadAction::OPTIONS,
            &[
                (0, "Download All Files"),
                (1, "Download All Available Files"),
                (2, "Manually Select Files"),
            ]
        );
    }

    #[test]
    fn serializes_as_integers() {
        assert_eq!(serde_json::to_string(&TorrentStatus::Error).unwrap(), "99");
        assert_eq!(
            serde_json::from_str::<Provider>("4").unwrap(),
            Provider::DebridLink
        );
        assert!(serde_json::from_str::<Provider>("\"TorBox\"").is_err());
    }

    #[test]
    fn first_variant_is_default() {
        assert_eq!(TorrentStatus::default(), TorrentStatus::Queued);
        assert_eq!(Provider::default(), Provider::RealDebrid);
    }

    #[test]
    fn converts_into_integers() {
        assert_eq!(i32::from(TorrentStatus::Error), 99);
        assert_eq!(i64::from(Provider::Highway), 5);
    }
}
