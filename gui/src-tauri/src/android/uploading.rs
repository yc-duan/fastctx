//! Tauri's Android bridge carries a Uint8Array as JSON, never InvokeBody::Raw.

pub(crate) fn bytes(body: &tauri::ipc::InvokeBody) -> Result<Vec<u8>, String> {
    match body {
        tauri::ipc::InvokeBody::Raw(bytes) => Ok(bytes.clone()),
        tauri::ipc::InvokeBody::Json(serde_json::Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|number| u8::try_from(number).ok())
                    .ok_or_else(|| "the attachment contains a value that is not a byte".to_string())
            })
            .collect(),
        _ => Err("the attachment's bytes did not come as an Android byte array".into()),
    }
}
