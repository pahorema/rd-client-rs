//! The torrent lifecycle of rd-client: adding torrents to the debrid
//! provider, selecting files, downloading and unpacking them on the host,
//! completing, retrying and cleaning up, plus the background jobs that keep
//! it running.

pub mod copy_added;
pub mod disk_space;
pub mod tracker_list;
