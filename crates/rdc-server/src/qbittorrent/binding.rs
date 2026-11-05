use std::collections::BTreeMap;

use axum::{
    body::to_bytes,
    extract::{FromRequest, Multipart, Request},
    http::{Method, StatusCode, header},
};

use crate::http::ApiError;

#[derive(Default)]
pub(super) struct Params {
    values: BTreeMap<String, String>,
    pub(super) files: Vec<Vec<u8>>,
}

impl Params {
    pub(super) fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub(super) fn required(&self, name: &str) -> Result<&str, ApiError> {
        self.get(name)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| bad_request(format!("{name} is required")))
    }

    pub(super) fn integer(&self, name: &str) -> Result<Option<i32>, ApiError> {
        self.get(name)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| bad_request(format!("Invalid {name}")))
            })
            .transpose()
    }

    pub(super) fn boolean(&self, name: &str) -> Result<bool, ApiError> {
        match self.get(name) {
            None => Ok(false),
            Some(value) if value.eq_ignore_ascii_case("true") => Ok(true),
            Some(value) if value.eq_ignore_ascii_case("false") => Ok(false),
            Some(_) => Err(bad_request(format!("Invalid {name}"))),
        }
    }

    pub(super) async fn read(request: Request, form_only: bool) -> Result<Self, ApiError> {
        let mut result = Self::default();
        if request.method() == Method::GET && !form_only {
            result.decode(request.uri().query().unwrap_or_default().as_bytes());
            return Ok(result);
        }
        let content_type = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if content_type.starts_with("multipart/form-data") {
            let mut multipart = Multipart::from_request(request, &())
                .await
                .map_err(bad_request)?;
            while let Some(field) = multipart.next_field().await.map_err(bad_request)? {
                let name = field.name().unwrap_or_default().to_ascii_lowercase();
                let file = field.file_name().is_some();
                let bytes = field.bytes().await.map_err(bad_request)?;
                if file {
                    if !bytes.is_empty() {
                        result.files.push(bytes.to_vec());
                    }
                } else {
                    let value = String::from_utf8(bytes.to_vec()).map_err(bad_request)?;
                    result.values.entry(name).or_insert(value);
                }
            }
        } else if content_type.starts_with("application/x-www-form-urlencoded") {
            let bytes = to_bytes(request.into_body(), 30_000_000)
                .await
                .map_err(bad_request)?;
            result.decode(&bytes);
        }
        Ok(result)
    }

    fn decode(&mut self, bytes: &[u8]) {
        for (key, value) in url::form_urlencoded::parse(bytes) {
            self.values
                .entry(key.to_ascii_lowercase())
                .or_insert_with(|| value.into_owned());
        }
    }
}

pub(super) fn bad_request(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, error.to_string())
}

pub(super) fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}
