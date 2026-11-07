use std::collections::BTreeMap;

use axum::{
    body::Bytes,
    extract::{FromRequest, Multipart, Request, multipart::MultipartError},
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

    /// Reads the query string of a GET request, or the form body of any other request.
    pub(super) async fn read(request: Request) -> Result<Self, ApiError> {
        let mut result = Self::default();
        if request.method() == Method::GET {
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
                .map_err(|rejection| ApiError::new(rejection.status(), rejection.body_text()))?;
            while let Some(field) = multipart
                .next_field()
                .await
                .map_err(|error| multipart_error(&error))?
            {
                let name = field.name().unwrap_or_default().to_ascii_lowercase();
                let file = field.file_name().is_some();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|error| multipart_error(&error))?;
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
            // The extractor honours the route's body limit.
            let bytes = Bytes::from_request(request, &())
                .await
                .map_err(|rejection| ApiError::new(rejection.status(), rejection.body_text()))?;
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

/// Keeps the status of multipart failures, such as 413 for a body over the route's limit.
fn multipart_error(error: &MultipartError) -> ApiError {
    ApiError::new(error.status(), error.body_text())
}

pub(super) fn bad_request(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, error.to_string())
}
