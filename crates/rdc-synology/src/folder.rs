//! File Station folder creation (`SYNO.FileStation.CreateFolder`).

use serde::Deserialize;

use crate::wire::json_array;
use crate::{Error, Session};

/// Extra folder information requested on creation.
const ADDITIONAL: [&str; 3] = ["real_path", "owner", "time"];

/// A folder reported by File Station.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Folder {
    /// Whether the entry is a directory.
    pub isdir: bool,
    /// Folder name.
    pub name: Option<String>,
    /// Path starting with the shared folder, for example `/Media/Downloads`.
    pub path: Option<String>,
}

#[derive(Deserialize)]
struct Created {
    #[serde(default)]
    folders: Vec<Folder>,
}

impl Session {
    /// Creates the folder `path` (starting with a shared folder, for example
    /// `/Media/Downloads/Some.Release`), including missing parents. An existing folder is not
    /// an error.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] when `path` has no `/`, [`Error::Api`] when File Station
    /// refuses (for example missing permission on the share), or any other [`Error`] of the
    /// round trip.
    pub async fn create_folder(&self, path: &str) -> Result<Vec<Folder>, Error> {
        let (parent, name) = path
            .rsplit_once('/')
            .ok_or(Error::InvalidArgument("the folder path must contain a '/'"))?;
        let parents = json_array([parent]);
        let names = json_array([name]);
        let additional = json_array(ADDITIONAL);
        let created: Created = self
            .call(
                |apis| &apis.create_folder,
                "create",
                &[
                    ("folder_path", &parents),
                    ("name", &names),
                    ("force_parent", "true"),
                    ("additional", &additional),
                ],
            )
            .await?;
        Ok(created.folders)
    }
}
