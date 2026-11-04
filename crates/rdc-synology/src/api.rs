//! The DSM Web APIs this client uses and their discovered locations.

use std::collections::HashMap;

use serde::Deserialize;

/// API discovery.
pub(crate) const INFO_API: &str = "SYNO.API.Info";
/// Login and logout.
pub(crate) const AUTH_API: &str = "SYNO.API.Auth";
/// Download Station tasks.
pub(crate) const TASK_API: &str = "SYNO.DownloadStation2.Task";
/// Download Station server configuration.
pub(crate) const DOWNLOAD_STATION_INFO_API: &str = "SYNO.DownloadStation.Info";
/// File Station folder creation.
pub(crate) const CREATE_FOLDER_API: &str = "SYNO.FileStation.CreateFolder";

/// Value of the `session` parameter sent to Download Station APIs.
const DOWNLOAD_STATION_SESSION: &str = "DownloadStation";
/// Value of the `session` parameter sent to File Station APIs.
const FILE_STATION_SESSION: &str = "FileStation";

/// Where and how to call one API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Api {
    pub(crate) name: &'static str,
    /// CGI path relative to `<dsm>/webapi/`.
    pub(crate) path: String,
    pub(crate) version: u32,
    /// Value of the `session` parameter, for APIs that take one.
    pub(crate) session: Option<&'static str>,
}

impl Api {
    fn new(name: &'static str, path: &str, version: u32, session: Option<&'static str>) -> Self {
        Self {
            name,
            path: path.to_owned(),
            version,
            session,
        }
    }
}

/// The APIs used by this client. Names and versions are fixed; paths come from discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Apis {
    pub(crate) auth: Api,
    pub(crate) task: Api,
    pub(crate) download_station_info: Api,
    pub(crate) create_folder: Api,
}

impl Default for Apis {
    fn default() -> Self {
        Self {
            auth: Api::new(AUTH_API, "entry.cgi", 6, None),
            task: Api::new(
                TASK_API,
                "DownloadStation/task.cgi",
                1,
                Some(DOWNLOAD_STATION_SESSION),
            ),
            download_station_info: Api::new(
                DOWNLOAD_STATION_INFO_API,
                "DownloadStation/info.cgi",
                2,
                Some(DOWNLOAD_STATION_SESSION),
            ),
            create_folder: Api::new(
                CREATE_FOLDER_API,
                "entry.cgi",
                2,
                Some(FILE_STATION_SESSION),
            ),
        }
    }
}

impl Apis {
    /// The discovery API itself, which always lives at `query.cgi`.
    pub(crate) fn info() -> Api {
        Api::new(INFO_API, "query.cgi", 1, None)
    }

    /// Default APIs with each path replaced by the one DSM reported, when it reported one.
    pub(crate) fn discovered(descriptions: &HashMap<String, ApiDescription>) -> Self {
        let mut apis = Self::default();
        for api in [
            &mut apis.auth,
            &mut apis.task,
            &mut apis.download_station_info,
            &mut apis.create_folder,
        ] {
            if let Some(path) = descriptions
                .get(api.name)
                .and_then(|description| description.path.as_deref())
            {
                path.clone_into(&mut api.path);
            }
        }
        apis
    }
}

/// One entry of the `SYNO.API.Info` query response.
#[derive(Debug, Deserialize)]
pub(crate) struct ApiDescription {
    path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_replaces_paths_and_keeps_names_and_versions() {
        let descriptions: HashMap<String, ApiDescription> = serde_json::from_str(
            r#"{
                "SYNO.API.Auth": {"path": "auth.cgi", "minVersion": 1, "maxVersion": 7},
                "SYNO.DownloadStation2.Task": {"path": "entry.cgi", "minVersion": 1, "maxVersion": 2},
                "SYNO.FileStation.CreateFolder": {"minVersion": 1, "maxVersion": 2}
            }"#,
        )
        .unwrap();

        let apis = Apis::discovered(&descriptions);

        assert_eq!(apis.auth, Api::new(AUTH_API, "auth.cgi", 6, None));
        assert_eq!(
            apis.task,
            Api::new(TASK_API, "entry.cgi", 1, Some("DownloadStation"))
        );
        assert_eq!(
            apis.download_station_info,
            Apis::default().download_station_info
        );
        assert_eq!(apis.create_folder, Apis::default().create_folder);
    }
}
