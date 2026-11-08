use super::Error;
use serde_json::Value;
use std::time::Duration;

/// How long a model's answer may take to begin; a run's own deadline for
/// each request (`Limits::request_timeout`) is the one that holds.
const ANSWER_WITHIN: Duration = Duration::from_secs(600);

pub async fn post(endpoint: &str, headers: &[(&str, &str)], body: Value) -> Result<Value, Error> {
    let response = fastctx_net::web::post::json(endpoint, headers, body.to_string().into_bytes(), ANSWER_WITHIN)
        .await
        .map_err(|e| Error::protocol(e.why))?;
    let status = response.status;
    let retry_after = response.headers.get("retry-after").and_then(|v| v.to_str().ok()).and_then(|value| {
        value.parse::<u64>().ok().map(Duration::from_secs).or_else(|| {
            httpdate::parse_http_date(value)
                .ok()
                .map(|at| at.duration_since(std::time::SystemTime::now()).unwrap_or_default())
        })
    });
    let request_id = response
        .headers
        .get("x-request-id")
        .or_else(|| response.headers.get("request-id"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = response.bytes(1024 * 1024).await.map_err(Error::protocol)?;
    let parsed: Result<Value, _> = serde_json::from_slice(&bytes);
    if !(200..300).contains(&status) {
        let value = parsed.unwrap_or(Value::Null);
        let code = value.pointer("/error/code").and_then(Value::as_str).unwrap_or("");
        let kind = value.pointer("/error/type").and_then(Value::as_str).unwrap_or("");
        let mut reason = format!("{endpoint} answered HTTP {status}");
        let named: Vec<&str> = [kind, code].into_iter().filter(|part| !part.is_empty()).collect();
        if !named.is_empty() {
            reason.push_str(&format!(" ({})", named.join("/")));
        }
        if !request_id.is_empty() {
            reason.push_str(&format!(", request {request_id}"));
        }
        let body = crate::runtime::bounded(&without_credentials(&String::from_utf8_lossy(&bytes), headers), 2048);
        if !body.is_empty() {
            reason.push_str(&format!(": {body}"));
        } else if status == 404 {
            reason.push_str(": nothing is served at this address; check the model address in the sentinel settings");
        }
        let mut error = Error::protocol(reason);
        error.retryable = (status == 408 || status == 409 || status == 429 || status >= 500)
            && !matches!(code, "insufficient_quota" | "billing_hard_limit_reached" | "usage_limit_reached");
        error.retry_after = retry_after;
        return Err(error);
    }
    parsed.map_err(|e| Error::protocol(format!("invalid model JSON: {e}")))
}

/// `text` with the credentials sent in `headers` hidden, before anything
/// cuts it short: a provider may write the key back, and a key cut in half
/// would no longer be found.
fn without_credentials(text: &str, headers: &[(&str, &str)]) -> String {
    let mut text = text.trim().to_string();
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("authorization") || name.eq_ignore_ascii_case("x-api-key") {
            let secret = value.strip_prefix("Bearer ").unwrap_or(value).trim();
            if !secret.is_empty() {
                text = text.replace(secret, "***");
            }
        }
    }
    text
}

pub fn number(value: &Value, key: &str) -> Result<u64, Error> {
    value.get(key).and_then(Value::as_u64).ok_or_else(|| Error::protocol(format!("missing model usage: {key}")))
}
pub fn string(value: &Value, key: &str) -> Result<String, Error> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::protocol(format!("missing model field: {key}")))
}
