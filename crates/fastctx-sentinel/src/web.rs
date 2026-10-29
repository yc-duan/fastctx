//! Audited public-web reads and a replaceable search provider.
use crate::ports::{ReadResult, Web};
use fastctx_agent::{FutureResult, state::save};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

pub trait Search: Send + Sync {
    fn search<'a>(&'a self, query: &'a str) -> FutureResult<'a, ReadResult>;
}
pub struct Audit {
    directory: PathBuf,
    next: AtomicU64,
}
impl Audit {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory, next: AtomicU64::new(0) }
    }
    /// Records destination and non-secret payload before sending. A failed audit
    /// prevents the request; auth headers are never included in the payload.
    pub fn record(&self, method: &str, url: &str, payload: &Value) -> Result<(), String> {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let at =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
        save(
            &self.directory.join(format!("{at}-{}-{n}.json", std::process::id())),
            &json!({"at_ns":at.to_string(),"method":method,"url":url,"payload":payload}),
        )
    }
}
pub struct Tavily {
    pub endpoint: String,
    pub key: String,
    pub audit: Audit,
}
impl Search for Tavily {
    fn search<'a>(&'a self, query: &'a str) -> FutureResult<'a, ReadResult> {
        Box::pin(async move {
            if query.is_empty() || query.len() > 8192 {
                return Err("invalid search query".into());
            }
            let payload = json!({"query":query,"search_depth":"basic","max_results":5,"include_answer":false,"include_raw_content":false,"include_usage":true});
            self.audit.record("POST", &self.endpoint, &payload)?;
            let auth = format!("Bearer {}", self.key);
            let response = fastctx_net::web::post::json(
                &self.endpoint,
                &[("authorization", &auth)],
                payload.to_string().into_bytes(),
            )
            .await
            .map_err(|e| e.why)?;
            let status = response.status;
            let bytes = response.bytes(65_536).await?;
            if !(200..300).contains(&status) {
                return Err(format!("search HTTP {status}: {}", String::from_utf8_lossy(&bytes)));
            }
            let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if !value["results"].is_array() {
                return Err("search returned no results array".into());
            }
            Ok(ReadResult { text: value.to_string(), exit_code: None, full_path: None })
        })
    }
}
pub struct PublicWeb {
    pub search: Box<dyn Search>,
    pub audit: Audit,
}
impl Web for PublicWeb {
    fn fetch<'a>(&'a self, url: &'a str) -> FutureResult<'a, ReadResult> {
        Box::pin(async move {
            let mut url = url.to_string();
            for _ in 0..=5 {
                let parsed = public_url(&url)?;
                self.audit.record("GET", &url, &Value::Null)?;
                let response = fastctx_net::web::public::get(&url, "text/html,text/plain,application/json")
                    .await
                    .map_err(|e| e.why)?;
                if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                    let location = response
                        .headers
                        .get("location")
                        .and_then(|v| v.to_str().ok())
                        .ok_or("web redirect has no location")?;
                    url = parsed.join(location).map_err(|e| e.to_string())?.to_string();
                    continue;
                }
                let status = response.status;
                let bytes = response.bytes(65_536).await?;
                if !(200..300).contains(&status) {
                    return Err(format!("web HTTP {status}: {}", String::from_utf8_lossy(&bytes)));
                }
                return Ok(ReadResult {
                    text: String::from_utf8_lossy(&bytes).into_owned(),
                    exit_code: None,
                    full_path: None,
                });
            }
            Err("too many web redirects".into())
        })
    }
    fn search<'a>(&'a self, query: &'a str) -> FutureResult<'a, ReadResult> {
        self.search.search(query)
    }
}
fn public_url(text: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(text).map_err(|e| e.to_string())?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err("public web requires an HTTPS URL without credentials".into());
    }
    let host = url.host_str().ok_or("web URL has no host")?.trim_matches(['[', ']']);
    if host.eq_ignore_ascii_case("localhost") || !host.contains('.') && !host.contains(':') {
        return Err("web URL is not public".into());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        let private = match ip {
            std::net::IpAddr::V4(ip) => {
                ip.is_private()
                    || ip.is_loopback()
                    || ip.is_link_local()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_broadcast()
            }
            std::net::IpAddr::V6(ip) => {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local()
                    || ip.to_ipv4_mapped().is_some()
            }
        };
        if private {
            return Err("web URL is not public".into());
        }
    }
    Ok(url)
}
