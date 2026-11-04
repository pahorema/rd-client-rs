//! Download engines of rd-client.
//!
//! Every engine implements [`DownloadEngine`] (see its documentation for the contract) so a
//! caller can keep a heterogeneous set of active transfers. Inputs are plain values (links,
//! paths, configuration structs), so the crate does not depend on the domain model.
//! [`verify_file_size`] checks a finished file against the size the provider announced.

mod engine;
mod verify;

pub use self::engine::{
    CANCELLED_MESSAGE, Completion, CompletionSender, DownloadEngine, Outcome, Progress,
    STOPPED_UNEXPECTEDLY_MESSAGE, StartError, completion_channel,
};
pub use self::verify::{VerifyFileSizeError, verify_file_size};
