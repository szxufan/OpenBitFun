//! Shared JSON argument helpers for Tauri-shaped HostInvoke payloads.

use serde_json::Value;

/// Extract the `request` object from Tauri-style params (`{ request: { ... } }`).
/// Falls back to the top-level object when `request` is absent.
pub(crate) fn request_value(args: &Value) -> &Value {
    args.get("request").unwrap_or(args)
}

/// Look up a field by camelCase key, falling back to snake_case.
fn field<'a>(obj: &'a Value, camel: &str, snake: &str) -> Option<&'a Value> {
    obj.get(camel).or_else(|| obj.get(snake))
}

pub(crate) fn get_string(obj: &Value, camel: &str) -> Result<String, String> {
    let snake = camel_to_snake(camel);
    field(obj, camel, &snake)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Missing or invalid '{camel}' field"))
}

pub(crate) fn optional_string(obj: &Value, camel: &str) -> Option<String> {
    let snake = camel_to_snake(camel);
    field(obj, camel, &snake)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub(crate) fn optional_bool(obj: &Value, camel: &str) -> Option<bool> {
    let snake = camel_to_snake(camel);
    field(obj, camel, &snake).and_then(|v| v.as_bool())
}

pub(crate) fn get_string_vec(obj: &Value, camel: &str) -> Result<Vec<String>, String> {
    let snake = camel_to_snake(camel);
    let items = field(obj, camel, &snake)
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("Missing or invalid '{camel}' field"))?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("Invalid string entry in '{camel}'"))
        })
        .collect()
}

fn camel_to_snake(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 4);
    for (i, ch) in camel.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prefers_camel_then_snake() {
        let camel = json!({ "workspacePath": "/a" });
        let snake = json!({ "workspace_path": "/b" });
        assert_eq!(get_string(&camel, "workspacePath").unwrap(), "/a");
        assert_eq!(get_string(&snake, "workspacePath").unwrap(), "/b");
        assert_eq!(
            optional_string(&camel, "workspacePath").as_deref(),
            Some("/a")
        );
        assert_eq!(
            get_string_vec(&json!({ "workspaceIds": ["b", "a"] }), "workspaceIds").unwrap(),
            vec!["b".to_string(), "a".to_string()]
        );
        assert_eq!(
            get_string_vec(&json!({ "workspace_ids": ["b", "a"] }), "workspaceIds").unwrap(),
            vec!["b".to_string(), "a".to_string()]
        );
        assert!(get_string_vec(&json!({ "workspaceIds": ["b", 1] }), "workspaceIds").is_err());
    }
}
