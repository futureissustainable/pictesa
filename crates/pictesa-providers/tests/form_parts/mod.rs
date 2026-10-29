//! Reads what an adapter sent as `multipart/form-data` (or JSON), so contract tests can check the
//! documented field names whichever encoding the adapter picks.
#![allow(dead_code)]

use std::collections::BTreeMap;

use serde_json::Value;

use crate::common::Call;

/// One form field or file.
#[derive(Debug, Clone)]
pub struct Part {
    pub name: String,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

impl Part {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}

/// The parts of a `multipart/form-data` body, in order. Empty when the body is not multipart.
pub fn parts(call: &Call) -> Vec<Part> {
    let Some(boundary) = call.header("content-type").and_then(|t| t.split("boundary=").nth(1)).map(|b| b.trim_matches('"').to_string()) else {
        return Vec::new();
    };
    let delimiter = format!("--{boundary}").into_bytes();
    let mut out = Vec::new();
    for chunk in split(&call.body, &delimiter).into_iter().skip(1) {
        if chunk.starts_with(b"--") {
            break;
        }
        let chunk = chunk.strip_prefix(b"\r\n").unwrap_or(chunk);
        let Some(head_end) = find(chunk, b"\r\n\r\n") else { continue };
        let head = String::from_utf8_lossy(&chunk[..head_end]).into_owned();
        let mut data = chunk[head_end + 4..].to_vec();
        if data.ends_with(b"\r\n") {
            data.truncate(data.len() - 2);
        }
        let attr = |key: &str| head.split(';').map(str::trim).find_map(|kv| kv.strip_prefix(&format!("{key}=\""))).map(|v| v.trim_end_matches('"').to_string());
        let content_type = head.lines().find(|l| l.to_ascii_lowercase().starts_with("content-type:")).map(|l| l["content-type:".len()..].trim().to_string());
        out.push(Part { name: attr("name").unwrap_or_default(), file_name: attr("filename"), content_type, data });
    }
    out
}

/// Text fields by name, from a multipart form or a JSON object (JSON strings unquoted).
pub fn fields(call: &Call) -> BTreeMap<String, String> {
    let form = parts(call);
    if form.is_empty() {
        if let Value::Object(map) = call.json() {
            return map.into_iter().map(|(k, v)| (k, v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect();
        }
        return BTreeMap::new();
    }
    form.into_iter().filter(|p| p.file_name.is_none()).map(|p| (p.name.clone(), p.text())).collect()
}

/// File parts with this field name, in order.
pub fn files<'a>(form: &'a [Part], name: &str) -> Vec<&'a Part> {
    form.iter().filter(|p| p.name == name && p.file_name.is_some()).collect()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn split<'a>(mut hay: &'a [u8], delimiter: &[u8]) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    while let Some(i) = find(hay, delimiter) {
        out.push(&hay[..i]);
        hay = &hay[i + delimiter.len()..];
    }
    out.push(hay);
    out
}
