//! Download engines of rd-client.
//!
//! Every engine implements [`DownloadEngine`] (see its documentation for the contract) so a
//! caller can keep a heterogeneous set of active transfers:
//!
//! - [`HttpDownload`]: the internal multi-connection HTTP downloader;
//! - [`Aria2Download`]: hands the download to an aria2 daemon;
//! - [`SymlinkDownload`]: links a file of a mounted provider drive into the download folder.
//!
//! Inputs are plain values (links, paths, configuration structs), so the crate does not depend
//! on the domain model. [`verify_file_size`] checks a finished file against the size the
//! provider announced.

mod aria2;
mod bind;
mod engine;
mod http;
#[expect(dead_code, reason = "the remote engines built on it follow")]
mod remote;
mod symlink;
mod verify;

pub use self::aria2::{Aria2Config, Aria2Download, Aria2Request};
pub use self::bind::{is_local_unicast_address, resolve_bind_address};
pub use self::engine::{
    CANCELLED_MESSAGE, Completion, CompletionSender, DownloadEngine, Outcome, Progress,
    STOPPED_UNEXPECTEDLY_MESSAGE, StartError, completion_channel,
};
pub use self::http::{HttpDownload, InternalConfig};
pub use self::symlink::{SourceLookup, SymlinkDownload, SymlinkError, SymlinkRequest};
pub use self::verify::{VerifyFileSizeError, verify_file_size};
