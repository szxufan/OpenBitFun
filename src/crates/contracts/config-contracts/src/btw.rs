//! Host-owned defaults for newly submitted side-question sessions.
//!
//! Keep the stored object opaque so future selections survive older readers and
//! unrelated configuration writes. Interpret it only at editing/execution time.

use crate::model_pool::AgentModelSelection;
use crate::types::{AIConfig, ModelCapability};
use serde_json::Value;

pub fn default_btw_config() -> Value {
    serde_json::json!({ "model_selection": { "kind": "inherit" } })
}

pub fn model_selection(config: &Value) -> Result<AgentModelSelection, String> {
    let object = config.as_object().ok_or("Invalid ai.btw configuration")?;
    let Some(value) = object.get("model_selection") else {
        return Ok(AgentModelSelection::Inherit);
    };
    serde_json::from_value(value.clone())
        .map_err(|error| format!("Unsupported ai.btw.model_selection: {error}"))
}

pub fn validate_model_selection(ai: &AIConfig) -> Result<AgentModelSelection, String> {
    let selection = model_selection(&ai.btw)?;
    match &selection {
        AgentModelSelection::Inherit => {}
        AgentModelSelection::Model { model_ref } => {
            if !ai.resolve_model_selection(model_ref).is_some_and(|id| {
                ai.models.iter().any(|model| {
                    model.id == id
                        && model.enabled
                        && model.supports_capability(ModelCapability::TextChat)
                })
            }) {
                return Err(format!(
                    "BTW model is missing, disabled or cannot chat: {model_ref}"
                ));
            }
        }
        AgentModelSelection::Pool { pool_id } => {
            let pool = ai.model_pools.get(pool_id)?;
            if pool.member_model_ids.is_empty() {
                return Err(format!("BTW model pool is empty: {pool_id}"));
            }
        }
    }
    Ok(selection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_ai_defaults_to_inherit_and_round_trips() {
        let ai: AIConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(
            model_selection(&ai.btw).unwrap(),
            AgentModelSelection::Inherit
        );
        let round_trip: AIConfig =
            serde_json::from_value(serde_json::to_value(ai).unwrap()).unwrap();
        assert_eq!(
            model_selection(&round_trip.btw).unwrap(),
            AgentModelSelection::Inherit
        );
    }

    #[test]
    fn future_and_unavailable_choices_are_preserved() {
        for value in [
            serde_json::json!({"model_selection":{"kind":"future","target":"new"},"future":true}),
            serde_json::json!({"model_selection":{"kind":"model","model_ref":"deleted"}}),
            serde_json::json!({"model_selection":{"kind":"pool","pool_id":"deleted"}}),
            Value::Null,
        ] {
            let mut ai: AIConfig =
                serde_json::from_value(serde_json::json!({"btw":value})).unwrap();
            ai.allow_subagent_model_override = false;
            assert_eq!(serde_json::to_value(&ai).unwrap()["btw"], value);
            assert!(validate_model_selection(&ai).is_err());
        }
    }
}
