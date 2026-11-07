use rdc_store::{LoginOutcome, User, UserError};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::common::TestStore;

/// Version 2 hash of `Passw0rd!` (PBKDF2-HMAC-SHA1, 1000 iterations, salt
/// `10..1f`), computed independently with Python's `hashlib`.
const V2_HASH: &str = "ABAREhMUFRYXGBkaGxwdHh8D0JclqrQbeYk+aqPTC8X10EUr7PH3FRUb6YwVnRmreg==";

async fn column(pool: &SqlitePool, name: &str) -> Option<String> {
    let sql = format!(r#"SELECT CAST("{name}" AS TEXT) FROM "AspNetUsers""#);
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_one(pool)
        .await
        .expect("user column")
}

async fn execute(pool: &SqlitePool, sql: &'static str) {
    sqlx::raw_sql(sql).execute(pool).await.expect("sql runs");
}

fn rejected(error: UserError) -> String {
    match error {
        UserError::Rejected(message) => message,
        other => panic!("expected a rejection, got {other:?}"),
    }
}

async fn login(test: &TestStore, user_name: &str, password: &str) -> LoginOutcome {
    test.store
        .users()
        .verify_login(user_name, password)
        .await
        .expect("verify login")
}

#[tokio::test]
async fn creating_the_account_writes_a_complete_row() {
    let test = TestStore::new().await;
    assert_eq!(test.store.users().first().await.expect("first"), None);

    let user = test
        .store
        .users()
        .create("Admin", "secret123")
        .await
        .expect("create");
    assert_eq!(
        test.store.users().first().await.expect("first"),
        Some(user.clone())
    );
    assert_eq!(user.user_name.as_deref(), Some("Admin"));
    assert!(user.lockout_enabled && user.lockout_end.is_none());
    let id = Uuid::parse_str(&user.id).expect("uuid id");
    assert_eq!(user.id, id.to_string(), "lower-case id");

    let raw = test.raw().await;
    assert_eq!(
        column(&raw, "NormalizedUserName").await.as_deref(),
        Some("ADMIN")
    );
    let hash = column(&raw, "PasswordHash").await.expect("hash");
    assert!(
        hash.starts_with("AQAAAAIAAYagAAAAE"),
        "version 3 SHA-512 hash: {hash}"
    );
    let stamp = column(&raw, "SecurityStamp").await.expect("stamp");
    assert_eq!(Some(&stamp), user.security_stamp.as_ref());
    assert_eq!(stamp.len(), 32);
    let concurrency = column(&raw, "ConcurrencyStamp")
        .await
        .expect("concurrency stamp");
    assert_eq!(
        concurrency,
        Uuid::parse_str(&concurrency).expect("uuid").to_string()
    );
    for (name, value) in [
        ("Email", None),
        ("NormalizedEmail", None),
        ("EmailConfirmed", Some("0")),
        ("PhoneNumber", None),
        ("PhoneNumberConfirmed", Some("0")),
        ("TwoFactorEnabled", Some("0")),
        ("LockoutEnd", None),
        ("LockoutEnabled", Some("1")),
        ("AccessFailedCount", Some("0")),
    ] {
        assert_eq!(column(&raw, name).await.as_deref(), value, "column {name}");
    }
    let found = test
        .store
        .users()
        .security_stamp(&user.id)
        .await
        .expect("stamp");
    assert_eq!(found, Some(stamp));
    assert_eq!(
        test.store
            .users()
            .security_stamp("missing")
            .await
            .expect("stamp"),
        None
    );
}

#[tokio::test]
async fn creating_rejects_weak_passwords_and_bad_names() {
    let test = TestStore::new().await;
    let users = test.store.users();
    let cases = [
        ("admin", "abcdef1", "Passwords must be at least 8 characters."),
        (
            "admin",
            "password",
            "Passwords must have at least one digit ('0'-'9').",
        ),
        (
            "admin",
            "11111111",
            "Passwords must use at least 3 different characters.",
        ),
        (
            "ad min",
            "secret123",
            "Username 'ad min' is invalid, can only contain letters or digits.",
        ),
        (
            "",
            "secret123",
            "Username '' is invalid, can only contain letters or digits.",
        ),
        // The password is checked first.
        ("ad min", "abc", "Passwords must be at least 8 characters."),
    ];
    for (name, password, message) in cases {
        let error = users.create(name, password).await.expect_err("rejected");
        assert_eq!(
            rejected(error),
            message,
            "name {name:?}, password {password:?}"
        );
    }
    assert_eq!(users.first().await.expect("first"), None);

    users.create("admin", "secret123").await.expect("create");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_one_account_can_ever_be_created() {
    let test = TestStore::new().await;
    let mut creations = tokio::task::JoinSet::new();
    for name in ["alice", "bob", "carol", "dave", "erin", "frank"] {
        let store = test.store.clone();
        creations.spawn(async move { store.users().create(name, "secret123").await });
    }
    let created: Vec<User> = creations
        .join_all()
        .await
        .into_iter()
        .filter_map(Result::ok)
        .collect();
    let users = test.store.users();
    assert_eq!(created.len(), 1, "exactly one concurrent creation wins");
    assert_eq!(users.first().await.expect("first"), Some(created[0].clone()));
    let raw = test.raw().await;
    let count: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "AspNetUsers""#)
        .fetch_one(&raw)
        .await
        .expect("count");
    assert_eq!(count, 1);
    let error = users.create("ADMIN", "secret234").await.expect_err("exists");
    assert!(matches!(error, UserError::AccountExists), "{error:?}");
}

#[tokio::test]
async fn logins_match_names_case_insensitively() {
    let test = TestStore::new().await;
    let user = test
        .store
        .users()
        .create("Admin", "secret123")
        .await
        .expect("create");

    assert_eq!(
        login(&test, "aDMIN", "secret123").await,
        LoginOutcome::Success(user)
    );
    for (name, password) in [
        ("admin", "secret234"),
        ("other", "secret123"),
        ("", "secret123"),
        ("admin", " "),
    ] {
        assert_eq!(
            login(&test, name, password).await,
            LoginOutcome::Failed,
            "{name:?}"
        );
    }
}

#[tokio::test]
async fn locked_out_accounts_cannot_log_in_until_the_lockout_ends() {
    let test = TestStore::new().await;
    test.store
        .users()
        .create("admin", "secret123")
        .await
        .expect("create");
    let raw = test.raw().await;

    execute(
        &raw,
        r#"UPDATE "AspNetUsers" SET "LockoutEnd" = '2999-01-01 00:00:00+00:00'"#,
    )
    .await;
    assert_eq!(
        login(&test, "admin", "secret123").await,
        LoginOutcome::LockedOut
    );
    execute(&raw, r#"UPDATE "AspNetUsers" SET "LockoutEnabled" = 0"#).await;
    assert!(matches!(
        login(&test, "admin", "secret123").await,
        LoginOutcome::Success(_)
    ));
    execute(
        &raw,
        r#"UPDATE "AspNetUsers" SET "LockoutEnabled" = 1, "LockoutEnd" = '2001-01-01 00:00:00+00:00'"#,
    )
    .await;
    assert!(matches!(
        login(&test, "admin", "secret123").await,
        LoginOutcome::Success(_)
    ));
}

#[tokio::test]
async fn logging_in_upgrades_weak_hashes_and_clears_failed_attempts() {
    let test = TestStore::new().await;
    let user = test
        .store
        .users()
        .create("admin", "secret123")
        .await
        .expect("create");
    let raw = test.raw().await;
    sqlx::query(r#"UPDATE "AspNetUsers" SET "PasswordHash" = ?, "AccessFailedCount" = 3"#)
        .bind(V2_HASH)
        .execute(&raw)
        .await
        .expect("store a version 2 hash");
    let concurrency = column(&raw, "ConcurrencyStamp").await;

    let LoginOutcome::Success(upgraded) = login(&test, "admin", "Passw0rd!").await else {
        panic!("the version 2 hash matches");
    };
    assert_ne!(upgraded.security_stamp, user.security_stamp);
    assert_eq!(
        column(&raw, "SecurityStamp").await,
        upgraded.security_stamp,
        "the new stamp is stored"
    );
    assert_ne!(column(&raw, "ConcurrencyStamp").await, concurrency);
    let hash = column(&raw, "PasswordHash").await.expect("hash");
    assert!(
        hash.starts_with("AQAAAAIAAYagAAAAE"),
        "upgraded to version 3: {hash}"
    );
    assert_eq!(
        column(&raw, "AccessFailedCount").await.as_deref(),
        Some("0")
    );

    let again = login(&test, "admin", "Passw0rd!").await;
    assert_eq!(again, LoginOutcome::Success(upgraded.clone()));
    assert_eq!(
        column(&raw, "PasswordHash").await.as_deref(),
        Some(hash.as_str())
    );
}

#[tokio::test]
async fn updating_credentials_changes_name_and_password() {
    let test = TestStore::new().await;
    let users = test.store.users();
    let created = users.create("admin", "secret123").await.expect("create");

    let id = created.id.clone();
    let renamed = users
        .update_credentials(&id, "secret123", "Owner", "")
        .await
        .expect("rename");
    assert_eq!(renamed.user_name.as_deref(), Some("Owner"));
    assert_ne!(
        renamed.security_stamp, created.security_stamp,
        "names end sessions"
    );
    assert_eq!(login(&test, "admin", "secret123").await, LoginOutcome::Failed);
    assert!(matches!(
        login(&test, "owner", "secret123").await,
        LoginOutcome::Success(_)
    ));

    let changed = users
        .update_credentials(&id, "secret123", " ", "better234")
        .await
        .expect("new password");
    assert_eq!(changed.user_name.as_deref(), Some("Owner"));
    assert_ne!(
        changed.security_stamp, renamed.security_stamp,
        "passwords end sessions"
    );
    assert_eq!(users.first().await.expect("first"), Some(changed.clone()));
    assert_eq!(login(&test, "owner", "secret123").await, LoginOutcome::Failed);
    assert_eq!(
        login(&test, "owner", "better234").await,
        LoginOutcome::Success(changed.clone())
    );

    let unchanged = users
        .update_credentials(&id, "better234", "", "")
        .await
        .expect("nothing to change");
    assert_eq!(unchanged, changed);
    let unchanged = users
        .update_credentials(&id, "better234", "Owner", "")
        .await
        .expect("unchanged name");
    assert_eq!(unchanged, changed);
    assert_eq!(users.get(&id).await.expect("get"), Some(changed));
    assert_eq!(users.get("missing").await.expect("get"), None);
}

#[tokio::test]
async fn rotating_the_security_stamp_ends_sessions_but_keeps_credentials() {
    let test = TestStore::new().await;
    let users = test.store.users();
    let created = users.create("admin", "secret123").await.expect("create");
    assert!(
        users
            .rotate_security_stamp(&created.id)
            .await
            .expect("rotate")
    );
    let stamp = users.security_stamp(&created.id).await.expect("stamp");
    assert_ne!(stamp, created.security_stamp);
    assert!(matches!(
        login(&test, "admin", "secret123").await,
        LoginOutcome::Success(user) if user.security_stamp == stamp
    ));
    assert!(!users.rotate_security_stamp("missing").await.expect("rotate"));
}

#[tokio::test]
async fn rejected_credential_updates_change_nothing() {
    let test = TestStore::new().await;
    let users = test.store.users();
    let error = users
        .update_credentials("missing", "secret123", "admin", "secret123")
        .await
        .expect_err("no account");
    assert!(matches!(error, UserError::NoUser), "{error:?}");

    let created = users.create("admin", "secret123").await.expect("create");
    for current in ["", "secret124"] {
        let error = users
            .update_credentials(&created.id, current, "owner", "better234")
            .await
            .expect_err("wrong current password");
        assert!(matches!(error, UserError::IncorrectPassword), "{error:?}");
    }
    let error = users
        .update_credentials(&created.id, "secret123", "bad name", "better234")
        .await
        .expect_err("bad name");
    assert_eq!(
        rejected(error),
        "Username 'bad name' is invalid, can only contain letters or digits."
    );
    let error = users
        .update_credentials(&created.id, "secret123", "owner", "weak")
        .await
        .expect_err("weak password");
    assert_eq!(rejected(error), "Passwords must be at least 8 characters.");
    let user: Option<User> = users.first().await.expect("first");
    assert_eq!(user, Some(created));
    assert!(matches!(
        login(&test, "admin", "secret123").await,
        LoginOutcome::Success(_)
    ));
}

#[tokio::test]
async fn the_first_account_is_the_lowest_id() {
    let test = TestStore::new().await;
    test.store
        .users()
        .create("zed", "secret123")
        .await
        .expect("create");
    let raw = test.raw().await;
    execute(
        &raw,
        r#"INSERT INTO "AspNetUsers" ("Id", "UserName", "NormalizedUserName", "EmailConfirmed",
               "PhoneNumberConfirmed", "TwoFactorEnabled", "LockoutEnabled", "AccessFailedCount")
           VALUES ('00000000-0000-0000-0000-000000000000', 'first', 'FIRST', 0, 0, 0, 1, 0)"#,
    )
    .await;
    let first = test
        .store
        .users()
        .first()
        .await
        .expect("first")
        .expect("found");
    assert_eq!(first.user_name.as_deref(), Some("first"));
    assert_eq!(
        login(&test, "first", "anything123").await,
        LoginOutcome::Failed,
        "no password"
    );
}
