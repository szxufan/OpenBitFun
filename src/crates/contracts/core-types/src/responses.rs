//! Explicit Responses request preferences. Missing settings preserve HTTP/full replay.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponsesTransport {
    #[default]
    Http,
    Websocket,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponsesContextMode {
    #[default]
    Full,
    Incremental,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResponsesRequestSettings {
    pub transport: ResponsesTransport,
    pub context: ResponsesContextMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_preferences_preserve_legacy_transport_and_context() {
        let settings: ResponsesRequestSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, ResponsesRequestSettings::default());
        let old = serde_json::json!({"transport":"http","context":"full"});
        assert_eq!(serde_json::to_value(settings).unwrap(), old);
        assert_eq!(
            serde_json::from_value::<ResponsesRequestSettings>(old).unwrap(),
            settings
        );
    }
}
