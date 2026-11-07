//! The user account (`AspNetUsers` of the existing database layout).
//!
//! rd-client has a single account, created once and never a second time. Names are
//! looked up through their upper-cased form; passwords are stored as
//! [`password`](crate::password) hashes. Every write rotates the row's
//! concurrency stamp and only applies when that stamp is unchanged; changing
//! the name or password also rotates the security stamp, invalidating sessions.

use jiff::Timestamp;
use sqlx::{SqlitePool, sqlite::SqliteRow};
use uuid::Uuid;

use crate::{
    error::{Result, StoreError},
    password::{self, Verification},
    row,
};

/// Shortest accepted password, in UTF-16 code units.
const MINIMUM_PASSWORD_LENGTH: usize = 5;
/// Fewest distinct characters (UTF-16 code units) in an accepted password.
const MINIMUM_DISTINCT_CHARACTERS: usize = 3;
/// Characters allowed besides ASCII letters and digits in user names.
const NAME_PUNCTUATION: &str = "-._@+";
/// Base32 alphabet of security stamps.
const STAMP_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

const OPTIMISTIC_CONCURRENCY_FAILURE: &str =
    "Optimistic concurrency failure, object has been modified.";

/// The user account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// Row id (a lower-case UUID for accounts created by rd-client).
    pub id: String,
    /// The name as typed when it was set.
    pub user_name: Option<String>,
    /// Changes whenever the name or password changes; sessions issued with an
    /// older stamp are no longer valid.
    pub security_stamp: Option<String>,
    /// The account cannot log in before this instant (when lockout is enabled).
    pub lockout_end: Option<Timestamp>,
    /// Whether [`lockout_end`](Self::lockout_end) applies.
    pub lockout_enabled: bool,
}

/// Failure of an account change.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UserError {
    /// The name or password was rejected; the message is shown to the user.
    #[error("{0}")]
    Rejected(String),
    /// There is no account to change.
    #[error("no user account exists")]
    NoUser,
    /// An account already exists, so no other can be created.
    #[error("an account already exists")]
    AccountExists,
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<sqlx::Error> for UserError {
    fn from(error: sqlx::Error) -> Self {
        Self::Store(error.into())
    }
}

/// Outcome of [`Users::verify_login`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    /// The credentials match; the account as stored after the login.
    Success(User),
    /// Unknown name, wrong password or blank input.
    Failed,
    /// The credentials may match but the account is locked out.
    LockedOut,
}

/// Account operations; obtained from [`Store::users`](crate::Store::users).
#[derive(Debug, Clone, Copy)]
pub struct Users<'store> {
    pool: &'store SqlitePool,
}

/// The stored row with its secrets, for internal use.
struct Account {
    user: User,
    password_hash: Option<String>,
    concurrency_stamp: Option<String>,
    access_failed_count: i64,
}

macro_rules! account_columns {
    () => {
        r#""Id", "UserName", "PasswordHash", "SecurityStamp", "ConcurrencyStamp", "LockoutEnd",
        "LockoutEnabled", "AccessFailedCount""#
    };
}

impl<'store> Users<'store> {
    pub(crate) fn new(pool: &'store SqlitePool) -> Self {
        Self { pool }
    }

    /// The account, if one was created.
    ///
    /// # Errors
    ///
    /// Fails on database errors and undecodable rows.
    pub async fn first(&self) -> Result<Option<User>> {
        Ok(self.first_account().await?.map(|account| account.user))
    }

    /// The security stamp of the account with this id; `None` when the
    /// account does not exist or has no stamp.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn security_stamp(&self, user_id: &str) -> Result<Option<String>> {
        let stamp: Option<Option<String>> =
            sqlx::query_scalar(r#"SELECT "SecurityStamp" FROM "AspNetUsers" WHERE "Id" = ?"#)
                .bind(user_id)
                .fetch_optional(self.pool)
                .await?;
        Ok(stamp.flatten())
    }

    /// Creates the account, unless one exists.
    ///
    /// The password needs at least 5 characters, one digit and 3 distinct
    /// characters; the name may only contain ASCII letters, digits and
    /// `-._@+`. Checking for an existing account and inserting the new one
    /// is a single statement, so concurrent calls create one account.
    ///
    /// # Errors
    ///
    /// Fails with [`UserError::Rejected`] and the first violated rule's
    /// message, [`UserError::AccountExists`] when an account exists, or on
    /// database errors.
    pub async fn create(&self, user_name: &str, password: &str) -> Result<User, UserError> {
        validate_password(password)?;
        validate_user_name(user_name)?;
        let user = User {
            id: Uuid::new_v4().to_string(),
            user_name: Some(user_name.to_owned()),
            security_stamp: Some(new_security_stamp()),
            lockout_end: None,
            lockout_enabled: true,
        };
        let inserted = sqlx::query(
            r#"INSERT INTO "AspNetUsers" ("Id", "UserName", "NormalizedUserName", "Email",
                   "NormalizedEmail", "EmailConfirmed", "PasswordHash", "SecurityStamp",
                   "ConcurrencyStamp", "PhoneNumber", "PhoneNumberConfirmed", "TwoFactorEnabled",
                   "LockoutEnd", "LockoutEnabled", "AccessFailedCount")
               SELECT ?, ?, ?, NULL, NULL, 0, ?, ?, ?, NULL, 0, 0, NULL, 1, 0
               WHERE NOT EXISTS (SELECT 1 FROM "AspNetUsers")"#,
        )
        .bind(&user.id)
        .bind(user_name)
        .bind(normalize(user_name))
        .bind(password::hash(password))
        .bind(&user.security_stamp)
        .bind(Uuid::new_v4().to_string())
        .execute(self.pool)
        .await?;
        if inserted.rows_affected() == 0 {
            return Err(UserError::AccountExists);
        }
        Ok(user)
    }

    /// Checks a login attempt. Failures never count towards a lockout.
    ///
    /// A match against a weaker stored hash upgrades the hash (rotating the
    /// security stamp); a match resets the failed-attempt counter.
    ///
    /// # Errors
    ///
    /// Fails on database errors and undecodable rows.
    pub async fn verify_login(&self, user_name: &str, password: &str) -> Result<LoginOutcome> {
        if user_name.trim().is_empty() || password.trim().is_empty() {
            return Ok(LoginOutcome::Failed);
        }
        let Some(mut account) = self.find_by_name(user_name).await? else {
            return Ok(LoginOutcome::Failed);
        };
        let locked = account.user.lockout_enabled
            && account
                .user
                .lockout_end
                .is_some_and(|end| end >= Timestamp::now());
        if locked {
            return Ok(LoginOutcome::LockedOut);
        }
        let verification = account
            .password_hash
            .as_deref()
            .map_or(Verification::Failed, |hash| {
                password::verify(hash, password)
            });
        match verification {
            Verification::Failed => return Ok(LoginOutcome::Failed),
            Verification::Success => {}
            Verification::SuccessRehashNeeded => {
                // Like any login write, a concurrent change of the account
                // only skips the upgrade; the login itself succeeds.
                let stamp = new_security_stamp();
                let upgraded = self
                    .write(
                        &mut account,
                        r#""PasswordHash" = ?, "SecurityStamp" = ?"#,
                        &[password::hash(password), stamp.clone()],
                    )
                    .await?;
                if upgraded {
                    account.user.security_stamp = Some(stamp);
                }
            }
        }
        if account.access_failed_count != 0 {
            self.write(&mut account, r#""AccessFailedCount" = 0"#, &[])
                .await?;
        }
        Ok(LoginOutcome::Success(account.user))
    }

    /// Changes the name and password of the account; a blank name or
    /// password keeps the current one. Both are validated like in
    /// [`create`](Self::create) before anything is written.
    /// Changing either credential rotates the security stamp; supplying an
    /// unchanged name with a blank password leaves the account untouched.
    ///
    /// # Errors
    ///
    /// Fails with [`UserError::NoUser`] without an account,
    /// [`UserError::Rejected`] when a rule is violated or the account
    /// changed concurrently, or on database errors.
    pub async fn update_credentials(
        &self,
        user_name: &str,
        password: &str,
    ) -> Result<User, UserError> {
        let mut account = self.first_account().await?.ok_or(UserError::NoUser)?;
        let new_name =
            !user_name.trim().is_empty() && account.user.user_name.as_deref() != Some(user_name);
        let new_password = !password.trim().is_empty();
        if new_name {
            validate_user_name(user_name)?;
            if let Some(other) = self.find_by_name(user_name).await?
                && other.user.id != account.user.id
            {
                return Err(UserError::Rejected(format!(
                    "Username '{user_name}' is already taken."
                )));
            }
        }
        if new_password {
            validate_password(password)?;
        }
        let mut assignments = Vec::new();
        let mut values = Vec::new();
        if new_name {
            assignments.push(r#""UserName" = ?, "NormalizedUserName" = ?"#);
            values.extend([user_name.to_owned(), normalize(user_name)]);
        }
        if new_password {
            assignments.push(r#""PasswordHash" = ?"#);
            values.push(password::hash(password));
        }
        if assignments.is_empty() {
            return Ok(account.user);
        }
        let stamp = new_security_stamp();
        assignments.push(r#""SecurityStamp" = ?"#);
        values.push(stamp.clone());
        if !self
            .write(&mut account, &assignments.join(", "), &values)
            .await?
        {
            return Err(UserError::Rejected(
                OPTIMISTIC_CONCURRENCY_FAILURE.to_owned(),
            ));
        }
        if new_name {
            account.user.user_name = Some(user_name.to_owned());
        }
        account.user.security_stamp = Some(stamp);
        Ok(account.user)
    }

    async fn first_account(&self) -> Result<Option<Account>> {
        let row = sqlx::query(concat!(
            "SELECT ",
            account_columns!(),
            r#" FROM "AspNetUsers" ORDER BY "Id" LIMIT 1"#
        ))
        .fetch_optional(self.pool)
        .await?;
        row.as_ref().map(account_from_row).transpose()
    }

    async fn find_by_name(&self, user_name: &str) -> Result<Option<Account>> {
        let row = sqlx::query(concat!(
            "SELECT ",
            account_columns!(),
            r#" FROM "AspNetUsers" WHERE "NormalizedUserName" = ? LIMIT 1"#
        ))
        .bind(normalize(user_name))
        .fetch_optional(self.pool)
        .await?;
        row.as_ref().map(account_from_row).transpose()
    }

    /// Applies `assignments` (with `values` bound in order) and a new
    /// concurrency stamp when the row still has the stamp it was read with;
    /// returns whether it did.
    async fn write(
        &self,
        account: &mut Account,
        assignments: &str,
        values: &[String],
    ) -> Result<bool> {
        let separator = if assignments.is_empty() { "" } else { ", " };
        let sql = format!(
            r#"UPDATE "AspNetUsers" SET {assignments}{separator}"ConcurrencyStamp" = ?
               WHERE "Id" = ? AND "ConcurrencyStamp" IS ?"#
        );
        let stamp = Uuid::new_v4().to_string();
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        for value in values {
            query = query.bind(value);
        }
        let updated = query
            .bind(&stamp)
            .bind(&account.user.id)
            .bind(&account.concurrency_stamp)
            .execute(self.pool)
            .await?;
        let applied = updated.rows_affected() == 1;
        if applied {
            account.concurrency_stamp = Some(stamp);
        }
        Ok(applied)
    }
}

fn account_from_row(row: &SqliteRow) -> Result<Account> {
    Ok(Account {
        user: User {
            id: row::text(row, "Id")?,
            user_name: row::optional_text(row, "UserName")?,
            security_stamp: row::optional_text(row, "SecurityStamp")?,
            lockout_end: row::optional_timestamp(row, "LockoutEnd")?,
            lockout_enabled: row::flag(row, "LockoutEnabled")?,
        },
        password_hash: row::optional_text(row, "PasswordHash")?,
        concurrency_stamp: row::optional_text(row, "ConcurrencyStamp")?,
        access_failed_count: row::optional_long(row, "AccessFailedCount")?.unwrap_or_default(),
    })
}

/// The lookup form of a user name: every character upper-cased where that
/// maps it to a single character.
fn normalize(user_name: &str) -> String {
    user_name
        .chars()
        .map(|character| {
            let mut upper = character.to_uppercase();
            match (upper.next(), upper.next()) {
                (Some(single), None) => single,
                _ => character,
            }
        })
        .collect()
}

fn validate_password(password: &str) -> Result<(), UserError> {
    let units: Vec<u16> = password.encode_utf16().collect();
    let message = if password.trim().is_empty() || units.len() < MINIMUM_PASSWORD_LENGTH {
        format!("Passwords must be at least {MINIMUM_PASSWORD_LENGTH} characters.")
    } else if !password.bytes().any(|byte| byte.is_ascii_digit()) {
        "Passwords must have at least one digit ('0'-'9').".to_owned()
    } else if distinct(&units) < MINIMUM_DISTINCT_CHARACTERS {
        format!("Passwords must use at least {MINIMUM_DISTINCT_CHARACTERS} different characters.")
    } else {
        return Ok(());
    };
    Err(UserError::Rejected(message))
}

fn distinct(units: &[u16]) -> usize {
    let mut sorted = units.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.len()
}

fn validate_user_name(user_name: &str) -> Result<(), UserError> {
    let allowed =
        |character: char| character.is_ascii_alphanumeric() || NAME_PUNCTUATION.contains(character);
    if user_name.trim().is_empty() || !user_name.chars().all(allowed) {
        return Err(UserError::Rejected(format!(
            "Username '{user_name}' is invalid, can only contain letters or digits."
        )));
    }
    Ok(())
}

/// A new random security stamp: 20 random bytes in base32.
fn new_security_stamp() -> String {
    let mut bytes = [0; 20];
    getrandom::fill(&mut bytes).expect("the operating system provides random bytes");
    let mut stamp = String::with_capacity(32);
    for chunk in bytes.chunks(5) {
        let group = chunk
            .iter()
            .fold(0_u64, |group, &byte| (group << 8) | u64::from(byte));
        for shift in (0..8).rev() {
            let index = usize::try_from((group >> (shift * 5)) & 0x1f).expect("five bits");
            stamp.push(char::from(STAMP_ALPHABET[index]));
        }
    }
    stamp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_rules_report_the_first_violation() {
        let cases = [
            ("", "Passwords must be at least 5 characters."),
            ("     ", "Passwords must be at least 5 characters."),
            ("a1b2", "Passwords must be at least 5 characters."),
            (
                "abcdef",
                "Passwords must have at least one digit ('0'-'9').",
            ),
            (
                "11111",
                "Passwords must use at least 3 different characters.",
            ),
            (
                "1a1a1a",
                "Passwords must use at least 3 different characters.",
            ),
        ];
        for (password, message) in cases {
            let error = validate_password(password).expect_err(password);
            assert_eq!(error.to_string(), message, "password {password:?}");
        }
        for password in ["abc12", "1a1a1b", "pässwörd1"] {
            assert!(validate_password(password).is_ok(), "password {password:?}");
        }
    }

    #[test]
    fn user_names_allow_letters_digits_and_some_punctuation() {
        assert!(validate_user_name("Admin-1.x_y@host+tag").is_ok());
        for name in ["", " ", "with space", "émile", "a/b"] {
            let error = validate_user_name(name).expect_err(name);
            assert_eq!(
                error.to_string(),
                format!("Username '{name}' is invalid, can only contain letters or digits.")
            );
        }
    }

    #[test]
    fn names_are_normalized_to_upper_case() {
        assert_eq!(normalize("Admin@host"), "ADMIN@HOST");
        assert_eq!(normalize("straße"), "STRAßE");
    }

    #[test]
    fn security_stamps_are_32_base32_characters() {
        let stamp = new_security_stamp();
        assert_eq!(stamp.len(), 32);
        assert!(
            stamp.bytes().all(|byte| STAMP_ALPHABET.contains(&byte)),
            "{stamp}"
        );
        assert_ne!(stamp, new_security_stamp());
    }
}
