//! The initialization-based and per-request MCP wire contracts.

use serde_json::{Value, json};

pub const VERSION: &str = "2026-07-28";
pub const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
pub const CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
const CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

#[derive(Clone, Copy)]
pub enum Protocol {
    Legacy,
    PerRequest,
}

impl Protocol {
    /// Validates modern request metadata without remembering discovery or
    /// borrowing fields from another request. Initialization remains legacy.
    pub fn request(method: &str, params: &Value, initialized: bool) -> Result<Self, Value> {
        if method == "initialize" {
            return Ok(Self::Legacy);
        }
        let meta = params.get("_meta");
        let marked = meta.is_some_and(|meta| {
            [VERSION_KEY, CLIENT_INFO, CLIENT_CAPABILITIES].iter().any(|key| meta.get(key).is_some())
        });
        if !marked && method != "server/discover" && (initialized || method == "ping") {
            return Ok(Self::Legacy);
        }
        let version = meta
            .and_then(|meta| meta.get(VERSION_KEY))
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("_meta.io.modelcontextprotocol/protocolVersion must be a string"))?;
        if version != VERSION {
            return Err(json!({
                "code": -32022,
                "message": "unsupported protocol version",
                "data": { "supported": [VERSION], "requested": version }
            }));
        }
        if !meta.and_then(|meta| meta.get(CLIENT_CAPABILITIES)).is_some_and(Value::is_object) {
            return Err(invalid("_meta.io.modelcontextprotocol/clientCapabilities must be an object"));
        }
        if let Some(info) = meta.and_then(|meta| meta.get(CLIENT_INFO))
            && (!info.get("name").is_some_and(Value::is_string) || !info.get("version").is_some_and(Value::is_string))
        {
            return Err(invalid("_meta.io.modelcontextprotocol/clientInfo must contain string name and version"));
        }
        Ok(Self::PerRequest)
    }

    /// Adds only the fields required by the selected wire contract. A zero
    /// TTL marks descriptions as immediately stale because they depend on
    /// the current caller and machines.
    pub fn response(self, id: &Value, mut result: Value, cacheable: bool) -> Value {
        if matches!(self, Self::PerRequest) {
            result["resultType"] = json!("complete");
            result["_meta"] = json!({"io.modelcontextprotocol/serverInfo": server_info()});
            if cacheable {
                result["ttlMs"] = json!(0);
                result["cacheScope"] = json!("private");
            }
        }
        json!({ "jsonrpc": "2.0", "id": id, "result": result })
    }
}

pub fn server_info() -> Value {
    json!({ "name": "fastctx", "version": env!("CARGO_PKG_VERSION") })
}

pub fn discovery() -> Value {
    json!({ "supportedVersions": [VERSION], "capabilities": { "tools": {} } })
}

fn invalid(message: &str) -> Value {
    json!({ "code": -32602, "message": message })
}
