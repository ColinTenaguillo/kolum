//! HTTP to Jira: authentication, a cap on requests in flight, and
//! `Retry-After` honoured on 429 before giving up.

use kolum_domain::{DomainError, Result};
pub use reqwest::Method;
use reqwest::header::{ACCEPT, AUTHORIZATION, RETRY_AFTER};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Jira Cloud's rate limits are per user; a handful in flight is what the
/// web app itself uses.
const IN_FLIGHT: usize = 4;
const RETRIES: u32 = 2;
/// A longer wait than this is reported rather than slept through: the
/// user is looking at a frozen spinner meanwhile.
const MAX_WAIT_SECS: u64 = 30;

pub struct Client {
    http: reqwest::Client,
    base: String,
    auth: String,
    limit: Semaphore,
}

impl Client {
    pub fn new(base: &str, auth: &str) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("kolum/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Client {
            http,
            base: base.to_string(),
            auth: auth.to_string(),
            limit: Semaphore::new(IN_FLIGHT),
        }
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let value = self.request(Method::GET, path, query, None).await?;
        serde_json::from_value(value).map_err(|e| DomainError::Transport(format!("{path}: {e}")))
    }

    /// The bytes at an absolute URL: an avatar. The credentials go only to
    /// this Jira; an avatar served from elsewhere (a CDN, Gravatar) gets a
    /// plain request.
    pub async fn bytes(&self, url: &str) -> Result<Vec<u8>> {
        let mut request = self.http.get(url);
        if same_site(&self.base, url) {
            request = request.header(AUTHORIZATION, &self.auth);
        }
        let response =
            request.send().await.map_err(|e| DomainError::Transport(format!("{url}: {e}")))?;
        let status = response.status().as_u16();
        let bytes =
            response.bytes().await.map_err(|e| DomainError::Transport(format!("{url}: {e}")))?;
        if (200..300).contains(&status) {
            Ok(bytes.to_vec())
        } else {
            Err(status_error(status, &String::from_utf8_lossy(&bytes), None))
        }
    }

    /// A write. Jira answers most of them with 204 and no body, which
    /// comes back as `Null`.
    pub async fn send(&self, method: Method, path: &str, body: &Value) -> Result<Value> {
        self.request(method, path, &[], Some(body)).await
    }

    /// A removal, with no body.
    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.request(Method::DELETE, path, &[], None).await
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut attempt = 0;
        loop {
            match self.once(&method, path, query, body).await {
                Err(DomainError::RateLimited { retry_after_secs: Some(secs) })
                    if attempt < RETRIES && secs <= MAX_WAIT_SECS =>
                {
                    tracing::info!("{path}: rate limited, retrying in {secs}s");
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                    attempt += 1;
                }
                answer => return answer,
            }
        }
    }

    /// One try, under the limit on requests in flight.
    async fn once(
        &self,
        method: &Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let permit =
            self.limit.acquire().await.map_err(|e| DomainError::Transport(e.to_string()))?;
        let mut request = self
            .http
            .request(method.clone(), format!("{}{path}", self.base))
            .header(AUTHORIZATION, &self.auth)
            .header(ACCEPT, "application/json")
            .query(query);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response =
            request.send().await.map_err(|e| DomainError::Transport(format!("{path}: {e}")))?;
        let status = response.status().as_u16();
        let retry = response.headers().get(RETRY_AFTER).and_then(|v| v.to_str().ok()?.parse().ok());
        let bytes =
            response.bytes().await.map_err(|e| DomainError::Transport(format!("{path}: {e}")))?;
        drop(permit);
        if !(200..300).contains(&status) {
            return Err(status_error(status, &String::from_utf8_lossy(&bytes), retry));
        }
        json_body(path, &bytes)
    }
}

/// A success's body as JSON, or `Null` when it has none.
fn json_body(path: &str, bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(bytes).map_err(|e| DomainError::Transport(format!("{path}: {e}")))
}

/// Whether `url` is on the site at `base`: same scheme, host and port. A
/// prefix test alone would let `https://acme.atlassian.net.evil.example`
/// through.
pub fn same_site(base: &str, url: &str) -> bool {
    let origin = |u: &str| -> Option<String> {
        let (scheme, rest) = u.split_once("://")?;
        Some(format!("{scheme}://{}", rest.split(['/', '?', '#']).next()?).to_lowercase())
    };
    origin(base).is_some_and(|b| origin(url) == Some(b))
}

/// Jira's own words when it gave any (`errorMessages`, `errors`), else the
/// start of the body.
fn message(body: &str) -> String {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let mut parts: Vec<String> = vec![];
    if let Some(v) = &parsed {
        let messages = v.get("errorMessages").and_then(|m| m.as_array()).into_iter().flatten();
        parts.extend(messages.filter_map(|m| m.as_str().map(String::from)));
        let fields = v.get("errors").and_then(|e| e.as_object()).into_iter().flatten();
        parts.extend(fields.filter_map(|(k, m)| Some(format!("{k}: {}", m.as_str()?))));
    }
    if parts.is_empty() { body.chars().take(200).collect() } else { parts.join("; ") }
}

pub fn status_error(status: u16, body: &str, retry_after_secs: Option<u64>) -> DomainError {
    let message = message(body);
    match status {
        401 | 403 => DomainError::Unauthorized(format!("HTTP {status}: {message}")),
        404 => DomainError::NotFound(message),
        409 => DomainError::Conflict(message),
        429 => DomainError::RateLimited { retry_after_secs },
        _ => DomainError::Transport(format!("HTTP {status}: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_stay_on_the_jira_site() {
        let base = "https://acme.atlassian.net";
        assert!(same_site(base, "https://acme.atlassian.net/secure/useravatar?id=1"));
        assert!(same_site(base, "https://ACME.atlassian.net"));
        assert!(!same_site(base, "https://acme.atlassian.net.evil.example/a"));
        assert!(!same_site(base, "https://secure.gravatar.com/avatar/x"));
        assert!(!same_site(base, "http://acme.atlassian.net/a"), "another scheme");
        assert!(!same_site(base, "https://acme.atlassian.net:8443/a"), "another port");
        assert!(!same_site(base, "not a url"));
    }

    #[test]
    fn statuses_become_errors_the_ui_can_act_on() {
        let jira = r#"{"errorMessages":["Issue does not exist"],"errors":{"jql":"bad field"}}"#;
        assert_eq!(
            status_error(404, jira, None),
            DomainError::NotFound("Issue does not exist; jql: bad field".into())
        );
        assert!(matches!(status_error(401, "", None), DomainError::Unauthorized(_)));
        assert!(matches!(status_error(403, "", None), DomainError::Unauthorized(_)));
        assert!(matches!(status_error(409, "", None), DomainError::Conflict(_)));
        assert_eq!(
            status_error(429, "", Some(5)),
            DomainError::RateLimited { retry_after_secs: Some(5) }
        );
        assert_eq!(
            status_error(502, "<html>bad gateway</html>", None),
            DomainError::Transport("HTTP 502: <html>bad gateway</html>".into())
        );
    }
}
