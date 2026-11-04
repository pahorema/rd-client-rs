//! Human-readable descriptions of DSM Web API error codes.

/// Codes shared by every API; they take precedence over the per-API tables.
const COMMON: &[(i64, &str)] = &[
    (100, "Unknown error"),
    (101, "No parameter of API, method or version"),
    (102, "The requested API does not exist"),
    (103, "The requested method does not exist"),
    (
        104,
        "The requested version does not support the functionality",
    ),
    (105, "The logged in session does not have permission"),
    (106, "Session timeout"),
    (107, "Session interrupted by duplicate login"),
    (108, "Failed to upload the file"),
    (
        109,
        "The network connection is unstable or the system is busy",
    ),
    (
        110,
        "The network connection is unstable or the system is busy",
    ),
    (
        111,
        "The network connection is unstable or the system is busy",
    ),
    (114, "Lost parameters for this API"),
    (115, "Not allowed to upload a file"),
    (116, "Not allowed to perform for a demo site"),
    (
        117,
        "The network connection is unstable or the system is busy",
    ),
    (
        118,
        "The network connection is unstable or the system is busy",
    ),
    (119, "SID not found"),
    (150, "Request source IP does not match the login IP"),
];

/// `SYNO.API.Auth`.
const AUTH: &[(i64, &str)] = &[
    (400, "No such account or incorrect password"),
    (401, "Account disabled"),
    (402, "Permission denied"),
    (403, "2-step verification code required"),
    (404, "Failed to authenticate 2-step verification code"),
    (406, "2-step verification is enforced for this account"),
    (407, "Blocked IP source"),
    (408, "Expired password cannot change"),
    (409, "Expired password"),
    (410, "Password must be changed"),
];

/// `SYNO.DownloadStation2.Task`.
const TASK: &[(i64, &str)] = &[
    (400, "File upload failed"),
    (401, "Max number of tasks reached"),
    (402, "Destination denied"),
    (403, "Destination does not exist"),
    (404, "Invalid task id"),
    (405, "Invalid task action"),
    (406, "No default destination"),
    (407, "Set destination failed"),
    (408, "File does not exist"),
];

/// Every `SYNO.FileStation.*` API.
const FILE_STATION: &[(i64, &str)] = &[
    (400, "Invalid parameter of file operation"),
    (401, "Unknown error of file operation"),
    (402, "System is too busy"),
    (403, "Invalid user does this file operation"),
    (404, "Invalid group does this file operation"),
    (405, "Invalid user and group does this file operation"),
    (
        406,
        "Can't get user/group information from the account server",
    ),
    (407, "Operation not permitted"),
    (408, "No such file or directory"),
    (409, "Non-supported file system"),
    (
        410,
        "Failed to connect internet-based file system (e.g., CIFS)",
    ),
    (411, "Read-only file system"),
    (412, "Filename too long in the non-encrypted file system"),
    (413, "Filename too long in the encrypted file system"),
    (414, "File already exists"),
    (415, "Disk quota exceeded"),
    (416, "No space left on device"),
    (417, "Input/output error"),
    (418, "Illegal name or path"),
    (419, "Illegal file name"),
    (420, "Illegal file name on FAT file system"),
    (421, "Device or resource busy"),
    (599, "No such task of the file operation"),
    (
        1100,
        "Failed to create a folder. More information in <errors> object",
    ),
    (
        1101,
        "The number of folders to the parent folder would exceed the system limitation",
    ),
];

/// Description of `code` returned by the API named `api`; empty when the code is unknown.
pub(crate) fn describe(api: &str, code: i64) -> &'static str {
    let table = if COMMON.iter().any(|&(known, _)| known == code) {
        COMMON
    } else if api == "SYNO.API.Auth" {
        AUTH
    } else if api == "SYNO.DownloadStation2.Task" {
        TASK
    } else if api.starts_with("SYNO.FileStation.") {
        FILE_STATION
    } else {
        return "";
    };
    table
        .iter()
        .find(|&&(known, _)| known == code)
        .map_or("", |&(_, description)| description)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTH_API: &str = "SYNO.API.Auth";
    const TASK_API: &str = "SYNO.DownloadStation2.Task";

    #[test]
    fn common_codes_apply_to_every_api() {
        assert_eq!(describe(AUTH_API, 119), "SID not found");
        assert_eq!(describe(TASK_API, 106), "Session timeout");
        assert_eq!(
            describe("SYNO.DownloadStation.Info", 105),
            "The logged in session does not have permission"
        );
    }

    #[test]
    fn api_specific_codes_depend_on_the_api() {
        assert_eq!(
            describe(AUTH_API, 400),
            "No such account or incorrect password"
        );
        assert_eq!(describe(TASK_API, 403), "Destination does not exist");
        assert_eq!(
            describe("SYNO.FileStation.CreateFolder", 408),
            "No such file or directory"
        );
        assert_eq!(
            describe("SYNO.FileStation.CreateFolder", 1100),
            "Failed to create a folder. More information in <errors> object"
        );
    }

    #[test]
    fn unknown_codes_have_an_empty_description() {
        assert_eq!(describe(AUTH_API, 499), "");
        assert_eq!(describe("SYNO.DownloadStation.Info", 400), "");
    }
}
