//! Typed access to the columns of a fetched row.

use jiff::Timestamp;
use sqlx::{Row, sqlite::SqliteRow};
use uuid::Uuid;

use crate::{
    codec,
    error::{Result, StoreError},
};

pub(crate) fn text(row: &SqliteRow, column: &'static str) -> Result<String> {
    Ok(row.try_get(column)?)
}

pub(crate) fn optional_text(row: &SqliteRow, column: &'static str) -> Result<Option<String>> {
    Ok(row.try_get(column)?)
}

pub(crate) fn optional_long(row: &SqliteRow, column: &'static str) -> Result<Option<i64>> {
    Ok(row.try_get(column)?)
}

pub(crate) fn int(row: &SqliteRow, column: &'static str) -> Result<i32> {
    let value: i64 = row.try_get(column)?;
    i32::try_from(value).map_err(|_| StoreError::invalid(column, &value))
}

pub(crate) fn optional_int(row: &SqliteRow, column: &'static str) -> Result<Option<i32>> {
    let value: Option<i64> = row.try_get(column)?;
    value
        .map(|value| i32::try_from(value).map_err(|_| StoreError::invalid(column, &value)))
        .transpose()
}

/// A boolean stored as 0/1.
pub(crate) fn flag(row: &SqliteRow, column: &'static str) -> Result<bool> {
    let value: i64 = row.try_get(column)?;
    Ok(value != 0)
}

pub(crate) fn uuid(row: &SqliteRow, column: &'static str) -> Result<Uuid> {
    codec::parse_uuid(column, &text(row, column)?)
}

pub(crate) fn timestamp(row: &SqliteRow, column: &'static str) -> Result<Timestamp> {
    codec::parse_timestamp(column, &text(row, column)?)
}

pub(crate) fn optional_timestamp(
    row: &SqliteRow,
    column: &'static str,
) -> Result<Option<Timestamp>> {
    optional_text(row, column)?
        .map(|text| codec::parse_timestamp(column, &text))
        .transpose()
}

/// An enumeration stored as its integer value.
pub(crate) fn enumeration<T: TryFrom<i64>>(row: &SqliteRow, column: &'static str) -> Result<T> {
    let value: i64 = row.try_get(column)?;
    T::try_from(value).map_err(|_| StoreError::invalid(column, &value))
}

pub(crate) fn optional_enumeration<T: TryFrom<i64>>(
    row: &SqliteRow,
    column: &'static str,
) -> Result<Option<T>> {
    let value: Option<i64> = row.try_get(column)?;
    value
        .map(|value| T::try_from(value).map_err(|_| StoreError::invalid(column, &value)))
        .transpose()
}
