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

/// Description of `code` returned by the API named `api`; empty when the code is unknown.
pub(crate) fn describe(api: &str, code: i64) -> &'static str {
    let table = if COMMON.iter().any(|&(known, _)| known == code) {
        COMMON
    } else if api == "SYNO.API.Auth" {
        AUTH
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
    }

    #[test]
    fn unknown_codes_have_an_empty_description() {
        assert_eq!(describe(AUTH_API, 499), "");
        assert_eq!(describe("SYNO.DownloadStation.Info", 400), "");
    }
}
