//! Gemini message format converter

use crate::types::{Message, ToolDefinition};
use log::warn;
use serde_json::{json, Map, Value};

pub struct GeminiMessageConverter;

impl GeminiMessageConverter {
    pub fn convert_messages(
        messages: Vec<Message>,
        model_name: &str,
    ) -> (Option<Value>, Vec<Value>) {
        let mut system_texts = Vec::new();
        let mut contents = Vec::new();
        let mut pending_tool_images = Vec::new();
        let is_gemini_3 = model_name.contains("gemini-3");

        for msg in messages {
            if msg.role != "tool" && !pending_tool_images.is_empty() {
                Self::push_content(
                    &mut contents,
                    "user",
                    std::mem::take(&mut pending_tool_images),
                );
            }
            match msg.role.as_str() {
                "system" => {
                    if let Some(content) = msg.content.filter(|content| !content.trim().is_empty())
                    {
                        system_texts.push(content);
                    }
                }
                "user" => {
                    let parts = Self::convert_content_parts(msg.content.as_deref(), false);
                    Self::push_content(&mut contents, "user", parts);
                }
                "assistant" => {
                    let mut parts = Vec::new();

                    let mut pending_thought_signature = msg
                        .thinking_signature
                        .filter(|value| !value.trim().is_empty());
                    let has_tool_calls = msg
                        .tool_calls
                        .as_ref()
                        .map(|tool_calls| !tool_calls.is_empty())
                        .unwrap_or(false);

                    if let Some(content) = msg
                        .content
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                    {
                        if !has_tool_calls {
                            if let Some(signature) = pending_thought_signature.take() {
                                parts.push(json!({
                                    "thoughtSignature": signature,
                                }));
                            }
                        }
                        parts.extend(Self::convert_content_parts(Some(content), true));
                    }

                    if let Some(tool_calls) = msg.tool_calls {
                        for (tool_call_index, tool_call) in tool_calls.into_iter().enumerate() {
                            let mut part = Map::new();
                            part.insert(
                                "functionCall".to_string(),
                                json!({
                                    "name": tool_call.name,
                                    "args": tool_call.arguments,
                                }),
                            );

                            match pending_thought_signature.take() {
                                Some(signature) => {
                                    part.insert(
                                        "thoughtSignature".to_string(),
                                        Value::String(signature),
                                    );
                                }
                                None if is_gemini_3 && tool_call_index == 0 => {
                                    part.insert(
                                        "thoughtSignature".to_string(),
                                        Value::String(
                                            "skip_thought_signature_validator".to_string(),
                                        ),
                                    );
                                }
                                None => {}
                            }

                            parts.push(Value::Object(part));
                        }
                    }

                    if let Some(signature) = pending_thought_signature {
                        parts.push(json!({
                            "thoughtSignature": signature,
                        }));
                    }

                    Self::push_content(&mut contents, "model", parts);
                }
                "tool" => {
                    let tool_name = msg.name.unwrap_or_default();
                    if tool_name.is_empty() {
                        warn!("Skipping Gemini tool response without tool name");
                        continue;
                    }

                    let is_error = msg.is_error.unwrap_or(false);
                    let response = if is_error {
                        let error_text = msg
                            .content
                            .as_deref()
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or("Tool execution failed");
                        json!({ "error": error_text })
                    } else {
                        Self::parse_tool_response(msg.content.as_deref())
                    };
                    let mut function_response = json!({ "name": tool_name, "response": response });
                    if let Some(images) = msg
                        .tool_image_attachments
                        .filter(|images| !images.is_empty())
                    {
                        let image_parts: Vec<Value> = images.into_iter().map(|image| json!({
                            "inlineData": { "mimeType": image.mime_type, "data": image.data_base64 }
                        })).collect();
                        // Gemini 3 accepts binary function-response parts. Older
                        // models need ordinary image parts after the tool batch.
                        // Never stringify bytes inside response JSON.
                        if is_gemini_3 {
                            function_response["parts"] = json!(image_parts);
                        } else {
                            pending_tool_images.push(json!({ "text": format!(
                                "Images returned by tool {} (tool_call_id: {}):", tool_name,
                                msg.tool_call_id.as_deref().unwrap_or("unknown")) }));
                            pending_tool_images.extend(image_parts);
                        }
                    }
                    let parts = vec![json!({ "functionResponse": function_response })];

                    Self::push_content(&mut contents, "user", parts);
                }
                _ => {
                    warn!("Unknown Gemini message role: {}", msg.role);
                }
            }
        }

        if !pending_tool_images.is_empty() {
            Self::push_content(&mut contents, "user", pending_tool_images);
        }

        let system_instruction = if system_texts.is_empty() {
            None
        } else {
            Some(json!({
                "parts": [{
                    "text": system_texts.join("\n\n")
                }]
            }))
        };

        (system_instruction, contents)
    }

    pub fn convert_tools(tools: Option<Vec<ToolDefinition>>) -> Option<Vec<Value>> {
        tools.and_then(|tool_defs| {
            let mut native_tools = Vec::new();
            let mut custom_tools = Vec::new();

            for tool in tool_defs {
                if let Some(native_tool) = Self::convert_native_tool(&tool) {
                    native_tools.push(native_tool);
                } else {
                    custom_tools.push(tool);
                }
            }

            // Gemini providers such as AIHubMix reject requests that mix built-in tools
            // with custom function declarations. When custom tools are present, keep all
            // tools in function-calling mode so OpenBitFun's local tool pipeline still works.
            let should_fallback_to_function_calling =
                !native_tools.is_empty() && !custom_tools.is_empty();

            let declarations: Vec<Value> = if should_fallback_to_function_calling {
                custom_tools
                    .into_iter()
                    .chain(
                        native_tools
                            .iter()
                            .cloned()
                            .filter_map(Self::convert_native_tool_to_custom_definition),
                    )
                    .map(Self::convert_custom_tool)
                    .collect()
            } else {
                custom_tools
                    .into_iter()
                    .map(Self::convert_custom_tool)
                    .collect()
            };

            let mut result_tools = if should_fallback_to_function_calling {
                Vec::new()
            } else {
                native_tools
            };

            if !declarations.is_empty() {
                result_tools.push(json!({
                    "functionDeclarations": declarations,
                }));
            }

            if result_tools.is_empty() {
                None
            } else {
                Some(result_tools)
            }
        })
    }

    pub fn sanitize_schema(value: Value) -> Value {
        Self::strip_unsupported_schema_fields(value)
    }

    fn convert_native_tool(tool: &ToolDefinition) -> Option<Value> {
        let native_name = Self::native_tool_name(&tool.name)?;
        let config = Self::native_tool_config(&tool.parameters);
        Some(json!({
            native_name: config,
        }))
    }

    fn convert_native_tool_to_custom_definition(native_tool: Value) -> Option<ToolDefinition> {
        let map = native_tool.as_object()?;
        let (name, _config) = map.iter().next()?;

        Some(ToolDefinition {
            name: Self::native_tool_fallback_name(name).to_string(),
            description: Self::native_tool_fallback_description(name).to_string(),
            parameters: Self::native_tool_fallback_schema(name),
        })
    }

    fn convert_custom_tool(tool: ToolDefinition) -> Value {
        let parameters = Self::sanitize_schema(tool.parameters);
        json!({
            "name": tool.name,
            "description": tool.description,
            "parameters": parameters,
        })
    }

    fn native_tool_name(tool_name: &str) -> Option<&'static str> {
        match tool_name {
            "WebSearch" | "googleSearch" | "GoogleSearch" => Some("googleSearch"),
            "WebFetch" | "urlContext" | "UrlContext" | "URLContext" => Some("urlContext"),
            "googleSearchRetrieval" | "GoogleSearchRetrieval" => Some("googleSearchRetrieval"),
            "codeExecution" | "CodeExecution" => Some("codeExecution"),
            _ => None,
        }
    }

    fn native_tool_fallback_name(native_name: &str) -> &'static str {
        match native_name {
            "googleSearch" => "WebSearch",
            "urlContext" => "WebFetch",
            "googleSearchRetrieval" => "googleSearchRetrieval",
            "codeExecution" => "codeExecution",
            _ => "unknown_native_tool",
        }
    }

    fn native_tool_fallback_description(native_name: &str) -> &'static str {
        match native_name {
            "googleSearch" => "Search the web for up-to-date information.",
            "urlContext" => "Fetch content from a URL for context.",
            "googleSearchRetrieval" => "Retrieve grounded results from Google Search.",
            "codeExecution" => "Execute model-generated code and return the result.",
            _ => "Gemini native tool fallback.",
        }
    }

    fn native_tool_fallback_schema(native_name: &str) -> Value {
        match native_name {
            "googleSearch" | "googleSearchRetrieval" => json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                    }
                },
                "required": ["query"]
            }),
            "urlContext" => json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                    }
                },
                "required": ["url"]
            }),
            "codeExecution" => json!({
                "type": "object",
                "properties": {}
            }),
            _ => json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    fn native_tool_config(parameters: &Value) -> Value {
        if Self::looks_like_schema(parameters) {
            json!({})
        } else {
            match parameters {
                Value::Object(map) if !map.is_empty() => parameters.clone(),
                _ => json!({}),
            }
        }
    }

    fn looks_like_schema(parameters: &Value) -> bool {
        let Some(map) = parameters.as_object() else {
            return false;
        };

        map.contains_key("type")
            || map.contains_key("properties")
            || map.contains_key("required")
            || map.contains_key("$schema")
            || map.contains_key("items")
            || map.contains_key("allOf")
            || map.contains_key("anyOf")
            || map.contains_key("oneOf")
            || map.contains_key("enum")
            || map.contains_key("nullable")
            || map.contains_key("format")
    }

    fn push_content(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
        if parts.is_empty() {
            return;
        }

        if let Some(last) = contents.last_mut() {
            let last_role = last.get("role").and_then(Value::as_str).unwrap_or_default();
            if last_role == role {
                if let Some(existing_parts) = last.get_mut("parts").and_then(Value::as_array_mut) {
                    existing_parts.extend(parts);
                    return;
                }
            }
        }

        contents.push(json!({
            "role": role,
            "parts": parts,
        }));
    }

    fn convert_content_parts(content: Option<&str>, is_model_role: bool) -> Vec<Value> {
        let Some(content) = content else {
            return Vec::new();
        };

        if content.trim().is_empty() {
            return Vec::new();
        }

        let Ok(Value::Array(items)) = serde_json::from_str::<Value>(content) else {
            return vec![json!({ "text": content })];
        };
        if !Self::is_content_parts(&items, is_model_role) {
            return vec![json!({ "text": content })];
        }
        let parts: Vec<_> = items
            .iter()
            .filter_map(|item| {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    return (!text.is_empty()).then(|| json!({ "text": text }));
                }
                let (mime, data) = Self::inline_image_payload(item)?;
                Some(json!({"inlineData": {"mimeType": mime, "data": data}}))
            })
            .collect();
        if parts.is_empty() {
            vec![json!({ "text": content })]
        } else {
            parts
        }
    }

    pub(crate) fn is_user_content_parts(items: &[Value]) -> bool {
        Self::is_content_parts(items, false)
    }

    fn is_content_parts(items: &[Value], is_model_role: bool) -> bool {
        // A bare array of objects with a text property can be ordinary user JSON.
        items
            .iter()
            .any(|item| item.get("type").is_some() || Self::inline_image_payload(item).is_some())
            && items.iter().all(|item| {
                let Some(fields) = item.as_object() else {
                    return false;
                };
                let kind = item["type"].as_str();
                let text = match kind {
                    Some("text" | "input_text" | "output_text") => fields.len() == 2,
                    None if !is_model_role => fields.len() == 1,
                    _ => false,
                } && item["text"].is_string();
                text || (!is_model_role && Self::inline_image_payload(item).is_some())
            })
    }

    fn inline_image_payload(item: &Value) -> Option<(&str, &str)> {
        let fields = item.as_object()?;
        let kind = item.get("type");
        let payload = match kind {
            None if fields.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "inlineData" | "inline_data" | "mediaResolution" | "media_resolution"
                )
            }) =>
            {
                item.get("inlineData").or_else(|| item.get("inline_data"))?
            }
            Some(kind) if kind == "image" && fields.len() == 2 => item.get("source")?,
            Some(kind)
                if (kind == "image_url" || kind == "input_image")
                    && fields
                        .keys()
                        .all(|key| matches!(key.as_str(), "type" | "image_url" | "detail")) =>
            {
                let value = item.get("image_url")?;
                let url = value.as_str().or_else(|| {
                    let fields = value.as_object()?;
                    fields
                        .keys()
                        .all(|key| matches!(key.as_str(), "url" | "detail"))
                        .then(|| value["url"].as_str())
                        .flatten()
                })?;
                let (mime, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
                return (!mime.is_empty() && !data.is_empty()).then_some((mime, data));
            }
            _ => return None,
        };
        let fields = payload.as_object()?;
        if !fields.keys().all(|key| {
            matches!(
                key.as_str(),
                "mimeType" | "mime_type" | "media_type" | "data"
            ) || (kind.is_some() && key == "type" && payload["type"] == "base64")
        }) {
            return None;
        }
        let mime = payload
            .get("mimeType")
            .or_else(|| payload.get("mime_type"))
            .or_else(|| payload.get("media_type"))?
            .as_str()?;
        let data = payload["data"].as_str()?;
        (!mime.is_empty() && !data.is_empty()).then_some((mime, data))
    }

    fn parse_tool_response(content: Option<&str>) -> Value {
        let Some(content) = content.filter(|value| !value.trim().is_empty()) else {
            return json!({ "content": "Tool execution completed" });
        };

        match serde_json::from_str::<Value>(content) {
            Ok(Value::Object(map)) => Value::Object(map),
            Ok(value) => json!({ "content": value }),
            Err(_) => json!({ "content": content }),
        }
    }

    fn strip_unsupported_schema_fields(value: Value) -> Value {
        match value {
            Value::Object(mut map) => {
                let all_of = map.remove("allOf");
                let any_of = map.remove("anyOf");
                let one_of = map.remove("oneOf");
                let (normalized_type, nullable_from_type) =
                    Self::normalize_schema_type(map.remove("type"));

                let mut sanitized = Map::new();
                for (key, value) in map {
                    if key == "properties" {
                        if let Value::Object(properties) = value {
                            sanitized.insert(
                                key,
                                Value::Object(
                                    properties
                                        .into_iter()
                                        .filter_map(|(name, schema)| match schema {
                                            // JSON Schema boolean forms: `true` accepts any
                                            // value, `false` forbids the property.
                                            Value::Bool(true) => Some((name, json!({}))),
                                            Value::Bool(false) => None,
                                            schema => Some((
                                                name,
                                                Self::strip_unsupported_schema_fields(schema),
                                            )),
                                        })
                                        .collect(),
                                ),
                            );
                        }
                        continue;
                    }

                    if key == "items" {
                        // Gemini only accepts a single item schema. Tuple (`items: [...]`)
                        // and boolean forms become an unconstrained element schema.
                        let items = match value {
                            Value::Object(_) => Self::strip_unsupported_schema_fields(value),
                            _ => json!({}),
                        };
                        sanitized.insert(key, items);
                        continue;
                    }

                    if Self::is_supported_schema_key(&key) {
                        sanitized.insert(key, Self::strip_unsupported_schema_fields(value));
                    }
                }

                if let Some(all_of) = all_of {
                    Self::merge_schema_variants(&mut sanitized, all_of, true);
                }

                let mut nullable = nullable_from_type;
                if let Some(any_of) = any_of {
                    nullable |= Self::merge_union_variants(&mut sanitized, any_of);
                }
                if let Some(one_of) = one_of {
                    nullable |= Self::merge_union_variants(&mut sanitized, one_of);
                }

                if let Some(schema_type) = normalized_type {
                    if schema_type == "array" && !sanitized.contains_key("items") {
                        // Gemini rejects array schemas without an item schema. An empty
                        // schema preserves the caller's unconstrained element type.
                        sanitized.insert("items".to_string(), json!({}));
                    }
                    sanitized.insert("type".to_string(), Value::String(schema_type));
                }
                if !sanitized.contains_key("type") {
                    // JSON Schema allows untyped schemas whose `properties` / `items`
                    // only constrain matching instances, but Gemini only accepts those
                    // keywords on OBJECT / ARRAY schemas. Infer the structural type.
                    if sanitized.contains_key("properties") {
                        sanitized.insert("type".to_string(), json!("object"));
                    } else if sanitized.contains_key("items") {
                        sanitized.insert("type".to_string(), json!("array"));
                    } else if sanitized
                        .get("enum")
                        .and_then(Value::as_array)
                        .is_some_and(|values| values.iter().all(Value::is_string))
                    {
                        sanitized.insert("type".to_string(), json!("string"));
                    }
                }
                Self::drop_keywords_incompatible_with_type(&mut sanitized);
                if nullable {
                    sanitized.insert("nullable".to_string(), Value::Bool(true));
                }

                Value::Object(sanitized)
            }
            Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .map(Self::strip_unsupported_schema_fields)
                    .collect(),
            ),
            other => other,
        }
    }

    /// Gemini validates schema keywords against the schema `type` and rejects a request
    /// that carries, for example, `items` on a STRING or `properties` on an ARRAY.
    /// Mixed-type unions and `type` arrays collapse to a single type here, so drop the
    /// keywords that the selected type cannot carry. This only loosens constraints.
    fn drop_keywords_incompatible_with_type(schema: &mut Map<String, Value>) {
        let Some(schema_type) = schema
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
        else {
            if schema
                .get("enum")
                .and_then(Value::as_array)
                .is_some_and(|values| !values.iter().all(Value::is_string))
            {
                schema.remove("enum");
            }
            return;
        };
        let is_number = matches!(schema_type.as_str(), "number" | "integer");

        if schema_type == "object" {
            let property_names: Option<Vec<String>> = schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|properties| properties.keys().cloned().collect());
            // Gemini rejects `required` entries that are not declared in `properties`.
            match (schema.get_mut("required"), property_names) {
                (Some(Value::Array(required)), Some(names)) => {
                    required.retain(|name| {
                        name.as_str()
                            .is_some_and(|name| names.iter().any(|known| known == name))
                    });
                    if required.is_empty() {
                        schema.remove("required");
                    }
                }
                (Some(_), _) => {
                    schema.remove("required");
                }
                (None, _) => {}
            }
        } else {
            schema.remove("properties");
            schema.remove("required");
        }
        if schema_type != "array" {
            schema.remove("items");
            schema.remove("minItems");
            schema.remove("maxItems");
        }
        if schema_type != "string" {
            schema.remove("minLength");
            schema.remove("maxLength");
            schema.remove("pattern");
        }
        if !is_number {
            schema.remove("minimum");
            schema.remove("maximum");
        }

        if let Some(values) = schema.get_mut("enum") {
            // Gemini only accepts string enums on STRING schemas.
            let keep = schema_type == "string"
                && match values {
                    Value::Array(values) => {
                        values.retain(Value::is_string);
                        !values.is_empty()
                    }
                    _ => false,
                };
            if !keep {
                schema.remove("enum");
            }
        }
    }

    fn is_supported_schema_key(key: &str) -> bool {
        matches!(
            key,
            "type"
                | "format"
                | "description"
                | "nullable"
                | "enum"
                | "items"
                | "properties"
                | "required"
                | "minItems"
                | "maxItems"
                | "minimum"
                | "maximum"
                | "minLength"
                | "maxLength"
                | "pattern"
        )
    }

    fn normalize_schema_type(type_value: Option<Value>) -> (Option<String>, bool) {
        match type_value {
            Some(Value::String(value)) if value != "null" => (Some(value), false),
            Some(Value::String(_)) => (None, true),
            Some(Value::Array(values)) => {
                let mut types = values
                    .into_iter()
                    .filter_map(|value| value.as_str().map(str::to_string));
                let mut nullable = false;
                let mut selected = None;

                for value in types.by_ref() {
                    if value == "null" {
                        nullable = true;
                    } else if selected.is_none() {
                        selected = Some(value);
                    }
                }

                (selected, nullable)
            }
            _ => (None, false),
        }
    }

    fn merge_union_variants(target: &mut Map<String, Value>, variants: Value) -> bool {
        let mut nullable = false;

        if let Value::Array(variants) = variants {
            for variant in variants {
                let sanitized = Self::strip_unsupported_schema_fields(variant);
                match sanitized {
                    Value::Object(map) => {
                        let is_null_only = map
                            .get("type")
                            .and_then(Value::as_str)
                            .map(|value| value == "null")
                            .unwrap_or(false)
                            && map.len() == 1;

                        if is_null_only {
                            nullable = true;
                            continue;
                        }

                        Self::merge_schema_map(target, map, false);
                    }
                    Value::String(value) if value == "null" => nullable = true,
                    _ => {}
                }
            }
        }

        nullable
    }

    fn merge_schema_variants(
        target: &mut Map<String, Value>,
        variants: Value,
        preserve_required: bool,
    ) {
        if let Value::Array(variants) = variants {
            for variant in variants {
                if let Value::Object(map) = Self::strip_unsupported_schema_fields(variant) {
                    Self::merge_schema_map(target, map, preserve_required);
                }
            }
        }
    }

    fn merge_schema_map(
        target: &mut Map<String, Value>,
        source: Map<String, Value>,
        preserve_required: bool,
    ) {
        for (key, value) in source {
            match key.as_str() {
                "properties" => {
                    if let Value::Object(source_props) = value {
                        let target_props = target
                            .entry(key)
                            .or_insert_with(|| Value::Object(Map::new()));
                        if let Value::Object(target_props) = target_props {
                            for (prop_key, prop_value) in source_props {
                                target_props.entry(prop_key).or_insert(prop_value);
                            }
                        }
                    }
                }
                "required" if preserve_required => {
                    if let Value::Array(source_required) = value {
                        let target_required = target
                            .entry(key)
                            .or_insert_with(|| Value::Array(Vec::new()));
                        if let Value::Array(target_required) = target_required {
                            for item in source_required {
                                if !target_required.contains(&item) {
                                    target_required.push(item);
                                }
                            }
                        }
                    }
                }
                "nullable" => {
                    if value.as_bool().unwrap_or(false) {
                        target.insert(key, Value::Bool(true));
                    }
                }
                "type" => {
                    target.entry(key).or_insert(value);
                }
                _ => {
                    target.entry(key).or_insert(value);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GeminiMessageConverter;
    use crate::types::{Message, ToolCall, ToolDefinition};
    use serde_json::json;

    #[test]
    fn converts_messages_to_gemini_format() {
        let messages = vec![
            Message::system("You are helpful".to_string()),
            Message::user("Hello".to_string()),
            Message {
                role: "assistant".to_string(),
                content: Some("Working on it".to_string()),
                reasoning_content: Some("Let me think".to_string()),
                thinking_signature: Some("sig_1".to_string()),
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "get_weather".to_string(),
                    arguments: json!({"city": "Beijing"}),
                    raw_arguments: None,
                }]),
                tool_call_id: None,
                name: None,
                is_error: None,
                tool_image_attachments: None,
                model_response_replay: None,
            },
            Message {
                role: "tool".to_string(),
                content: Some("Sunny".to_string()),
                reasoning_content: None,
                thinking_signature: None,
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                name: Some("get_weather".to_string()),
                is_error: None,
                tool_image_attachments: None,
                model_response_replay: None,
            },
        ];

        let (system_instruction, contents) =
            GeminiMessageConverter::convert_messages(messages, "gemini-2.5-pro");

        assert_eq!(
            system_instruction.unwrap()["parts"][0]["text"],
            json!("You are helpful")
        );
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], json!("user"));
        assert_eq!(contents[1]["role"], json!("model"));
        assert_eq!(contents[1]["parts"][0]["text"], json!("Working on it"));
        assert_eq!(
            contents[1]["parts"][1]["functionCall"]["name"],
            json!("get_weather")
        );
        assert_eq!(contents[1]["parts"][1]["thoughtSignature"], json!("sig_1"));
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["name"],
            json!("get_weather")
        );
    }

    #[test]
    fn injects_skip_signature_for_first_synthetic_gemini_3_tool_call() {
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: None,
            reasoning_content: None,
            thinking_signature: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "get_weather".to_string(),
                arguments: json!({"city": "Paris"}),
                raw_arguments: None,
            }]),
            tool_call_id: None,
            name: None,
            is_error: None,
            tool_image_attachments: None,
            model_response_replay: None,
        }];

        let (_, contents) =
            GeminiMessageConverter::convert_messages(messages, "gemini-3-flash-preview");

        assert_eq!(contents.len(), 1);
        assert_eq!(
            contents[0]["parts"][0]["thoughtSignature"],
            json!("skip_thought_signature_validator")
        );
    }

    #[test]
    fn native_user_images_stay_in_order_with_text_on_the_gemini_wire() {
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a1j8AAAAASUVORK5CYII=";
        let message = Message::user(
            json!([
                {"inline_data": {"mime_type": "image/png", "data": png}},
                {"text": "Compare the two images"},
                {"inlineData": {"mimeType": "image/png", "data": png}},
                {"text": "Report the differences"}
            ])
            .to_string(),
        );
        let (_, contents) = GeminiMessageConverter::convert_messages(vec![message], "gemini");
        assert_eq!(
            contents,
            vec![json!({"role": "user", "parts": [
                {"inlineData": {"mimeType": "image/png", "data": png}},
                {"text": "Compare the two images"},
                {"inlineData": {"mimeType": "image/png", "data": png}},
                {"text": "Report the differences"}
            ]})]
        );

        for literal in [
            r#"[{"text":"example JSON"}]"#,
            r#"[{"type":"text","text":""}]"#,
            r#"[{"id":7,"text":"first item"},{"inlineData":{"reference":"attachment-1"}}]"#,
        ] {
            let (_, contents) = GeminiMessageConverter::convert_messages(
                vec![Message::user(literal.into())],
                "gemini",
            );
            assert_eq!(contents[0]["parts"], json!([{"text": literal}]));
        }
    }

    #[test]
    fn converts_data_url_images_to_inline_data() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: Some(
                json!([
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": "data:image/png;base64,abc"
                        }
                    },
                    {
                        "type": "text",
                        "text": "Describe this image"
                    }
                ])
                .to_string(),
            ),
            reasoning_content: None,
            thinking_signature: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_error: None,
            tool_image_attachments: None,
            model_response_replay: None,
        }];

        let (_, contents) = GeminiMessageConverter::convert_messages(messages, "gemini-2.5-pro");

        assert_eq!(
            contents[0]["parts"][0]["inlineData"]["mimeType"],
            json!("image/png")
        );
        assert_eq!(
            contents[0]["parts"][1]["text"],
            json!("Describe this image")
        );
    }

    #[test]
    fn strips_unsupported_fields_from_tool_schema() {
        let tools = Some(vec![ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            parameters: json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {
                    "city": { "type": "string" },
                    "timezone": {
                        "type": ["string", "null"]
                    },
                    "link": {
                        "anyOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "url": { "type": "string" }
                                },
                                "required": ["url"]
                            },
                            { "type": "null" }
                        ]
                    },
                    "items": {
                        "allOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string" }
                                },
                                "required": ["name"]
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "count": { "type": "integer" }
                                },
                                "required": ["count"]
                            }
                        ]
                    }
                },
                "required": ["city"],
                "additionalProperties": false,
                "items": {
                    "type": "object",
                    "additionalProperties": false
                }
            }),
        }]);

        let converted = GeminiMessageConverter::convert_tools(tools).expect("converted tools");
        let schema = &converted[0]["functionDeclarations"][0]["parameters"];

        assert!(schema.get("$schema").is_none());
        assert!(schema.get("additionalProperties").is_none());
        assert!(schema["items"].get("additionalProperties").is_none());
        assert_eq!(schema["properties"]["timezone"]["type"], json!("string"));
        assert_eq!(schema["properties"]["timezone"]["nullable"], json!(true));
        assert_eq!(schema["properties"]["link"]["type"], json!("object"));
        assert_eq!(schema["properties"]["link"]["nullable"], json!(true));
        assert_eq!(schema["properties"]["items"]["type"], json!("object"));
        assert_eq!(
            schema["properties"]["items"]["required"],
            json!(["name", "count"])
        );
    }

    #[test]
    fn adds_items_to_array_schemas_without_restricting_elements() {
        let schema = GeminiMessageConverter::sanitize_schema(json!({
            "type": "object",
            "properties": {
                "value_array": { "type": "array" },
                "typed_array": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            }
        }));

        assert_eq!(schema["properties"]["value_array"]["items"], json!({}));
        assert_eq!(
            schema["properties"]["typed_array"]["items"]["type"],
            json!("string")
        );
    }

    #[test]
    fn infers_structural_type_for_untyped_properties_and_items() {
        let schema = GeminiMessageConverter::sanitize_schema(json!({
            "type": "object",
            "properties": {
                "paragraphs": {
                    "type": "array",
                    "items": {
                        "description": "String or paragraph object.",
                        "properties": { "text": { "type": "string" } },
                        "additionalProperties": true
                    }
                },
                "rows": {
                    "description": "2-D array or integer.",
                    "items": {
                        "type": "array",
                        "items": {
                            "properties": {
                                "value": {},
                                "formula": { "type": "string" }
                            }
                        }
                    }
                },
                "untyped_leaf": { "description": "Anything." }
            }
        }));

        let props = &schema["properties"];
        assert_eq!(props["paragraphs"]["items"]["type"], json!("object"));
        assert_eq!(props["rows"]["type"], json!("array"));
        assert_eq!(props["rows"]["items"]["type"], json!("array"));
        assert_eq!(props["rows"]["items"]["items"]["type"], json!("object"));
        assert!(props["untyped_leaf"].get("type").is_none());
        assert!(props["rows"]["items"]["items"]["properties"]["value"]
            .get("type")
            .is_none());
    }

    #[test]
    fn drops_keywords_that_conflict_with_the_collapsed_union_type() {
        let schema = GeminiMessageConverter::sanitize_schema(json!({
            "type": "object",
            "properties": {
                "string_or_list": {
                    "anyOf": [
                        { "type": "string", "minLength": 1 },
                        { "type": "array", "items": { "type": "string" }, "minItems": 1 }
                    ]
                },
                "list_or_string": {
                    "type": ["array", "string"],
                    "items": { "type": "string" },
                    "maxLength": 10
                },
                "level": { "type": "integer", "enum": [1, 2, 3], "minimum": 1 },
                "mode": { "type": ["string", "null"], "enum": ["fast", "slow", null] },
                "untyped_choice": { "enum": ["a", "b"] },
                "untyped_numeric_choice": { "enum": [1, 2] },
                "tuple": { "type": "array", "items": [{ "type": "string" }, { "type": "integer" }] },
                "any_value": true,
                "never": false
            },
            "required": ["string_or_list", "never", "missing"]
        }));

        let props = &schema["properties"];
        assert_eq!(props["string_or_list"]["type"], json!("string"));
        assert_eq!(props["string_or_list"]["minLength"], json!(1));
        assert!(props["string_or_list"].get("items").is_none());
        assert!(props["string_or_list"].get("minItems").is_none());

        assert_eq!(props["list_or_string"]["type"], json!("array"));
        assert_eq!(props["list_or_string"]["items"]["type"], json!("string"));
        assert!(props["list_or_string"].get("maxLength").is_none());

        assert_eq!(props["level"]["type"], json!("integer"));
        assert_eq!(props["level"]["minimum"], json!(1));
        assert!(props["level"].get("enum").is_none());

        assert_eq!(props["mode"]["type"], json!("string"));
        assert_eq!(props["mode"]["nullable"], json!(true));
        assert_eq!(props["mode"]["enum"], json!(["fast", "slow"]));

        assert_eq!(props["untyped_choice"]["type"], json!("string"));
        assert!(props["untyped_numeric_choice"].get("enum").is_none());
        assert!(props["untyped_numeric_choice"].get("type").is_none());

        assert_eq!(props["tuple"]["items"], json!({}));
        assert_eq!(props["any_value"], json!({}));
        assert!(props.get("never").is_none());
        assert_eq!(schema["required"], json!(["string_or_list"]));
    }

    #[test]
    fn maps_web_search_to_native_google_search_tool() {
        let tools = Some(vec![ToolDefinition {
            name: "WebSearch".to_string(),
            description: "Search the web".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
        }]);

        let converted = GeminiMessageConverter::convert_tools(tools).expect("converted tools");
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["googleSearch"], json!({}));
        assert!(converted[0].get("functionDeclarations").is_none());
    }

    #[test]
    fn falls_back_to_function_declarations_when_native_and_custom_tools_mix() {
        let tools = Some(vec![
            ToolDefinition {
                name: "WebSearch".to_string(),
                description: "Search the web".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    }
                }),
            },
            ToolDefinition {
                name: "get_weather".to_string(),
                description: "Get weather".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    },
                    "required": ["city"]
                }),
            },
        ]);

        let converted = GeminiMessageConverter::convert_tools(tools).expect("converted tools");
        assert_eq!(converted.len(), 1);
        assert!(converted[0].get("googleSearch").is_none());
        assert_eq!(
            converted[0]["functionDeclarations"][0]["name"],
            json!("get_weather")
        );
        assert_eq!(
            converted[0]["functionDeclarations"][1]["name"],
            json!("WebSearch")
        );
    }

    #[test]
    fn maps_web_fetch_to_native_url_context_tool() {
        let tools = Some(vec![ToolDefinition {
            name: "WebFetch".to_string(),
            description: "Fetch a URL".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" }
                },
                "required": ["url"]
            }),
        }]);

        let converted = GeminiMessageConverter::convert_tools(tools).expect("converted tools");
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["urlContext"], json!({}));
    }
}

#[cfg(test)]
mod tool_image_tests {
    use super::*;
    #[test]
    fn computer_use_images_reach_gemini_wire_without_splitting_parallel_results() {
        for model in ["gemini-2.5-pro", "gemini-3-pro"] {
            let messages: Vec<Message> = serde_json::from_value(json!([
                {"role":"tool","name":"ComputerUse","tool_call_id":"observe-1","content":"{\"screenshot_id\":\"shot-a\",\"image_width\":2,\"image_height\":1}","tool_image_attachments":[{"mime_type":"image/png","data_base64":"pixels-a"}]},
                {"role":"tool","name":"ComputerUse","tool_call_id":"observe-2","content":"{\"screenshot_id\":\"shot-b\",\"image_width\":4,\"image_height\":3}","tool_image_attachments":[{"mime_type":"image/png","data_base64":"pixels-b"}]}
            ])).unwrap();
            let (_, contents) = GeminiMessageConverter::convert_messages(messages, model);
            let parts = contents[0]["parts"].as_array().unwrap();
            assert_eq!(
                parts[0]["functionResponse"]["response"]["screenshot_id"],
                "shot-a"
            );
            assert_eq!(parts[1]["functionResponse"]["response"]["image_width"], 4);
            if model.contains("gemini-3") {
                assert_eq!(
                    parts[0]["functionResponse"]["parts"][0]["inlineData"]["data"],
                    "pixels-a"
                );
                assert_eq!(
                    parts[1]["functionResponse"]["parts"][0]["inlineData"]["data"],
                    "pixels-b"
                );
            } else {
                assert_eq!(parts[3]["inlineData"]["data"], "pixels-a");
                assert_eq!(parts[5]["inlineData"]["data"], "pixels-b");
            }
        }
    }
}
