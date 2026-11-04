use serde_json::{Value, json};
use wiremock::ResponseTemplate;

pub(crate) fn ok(data: &Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"success": true, "data": data}))
}
