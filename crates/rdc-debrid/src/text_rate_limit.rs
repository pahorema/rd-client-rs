//! Rate limits that providers only report in the text of an error.

use crate::error::DebridError;
use crate::http::DEFAULT_RATE_LIMIT_COOLDOWN;

/// Turns an error of an add call whose text mentions `slow_down` or
/// `rate limit exceeded` (in any case) into [`DebridError::RateLimited`]
/// with the default two-minute cooldown; other errors are returned as is.
pub(crate) fn classify_add_error(error: DebridError) -> DebridError {
    if matches!(error, DebridError::RateLimited { .. }) {
        return error;
    }
    let message = error.to_string();
    let lowercase = message.to_lowercase();
    if lowercase.contains("slow_down") || lowercase.contains("rate limit exceeded") {
        DebridError::RateLimited {
            message,
            retry_after: DEFAULT_RATE_LIMIT_COOLDOWN,
        }
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn slow_down_and_rate_limit_texts_become_rate_limits() {
        for text in ["slow_down", "Provider RATE LIMIT exceeded today"] {
            let error = classify_add_error(DebridError::Provider(text.to_owned()));

            assert_eq!(
                error.retry_after(),
                Some(DEFAULT_RATE_LIMIT_COOLDOWN),
                "{text}"
            );
            assert_eq!(error.to_string(), text);
        }
    }

    #[test]
    fn other_errors_are_unchanged() {
        let error = classify_add_error(DebridError::Provider("Slow down".to_owned()));

        assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
    }

    #[test]
    fn existing_rate_limits_keep_their_cooldown() {
        let error = classify_add_error(DebridError::RateLimited {
            message: "Provider host rate limit exceeded".to_owned(),
            retry_after: Duration::from_secs(5),
        });

        assert_eq!(error.retry_after(), Some(Duration::from_secs(5)));
    }
}
