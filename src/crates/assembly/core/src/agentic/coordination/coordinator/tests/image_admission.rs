use super::super::test_support::test_ai_config;
use super::super::ConversationCoordinator;
use crate::agentic::core::{SessionConfig, SessionModelBindingPolicy};
use crate::util::errors::OpenBitFunError;

#[test]
fn input_image_count_admission_ignores_stale_pool_pointers_for_fixed_models() {
    let mut config = SessionConfig {
        model_id: Some("fixed-vision".into()),
        model_pool_id: Some("stale-pool".into()),
        model_pool_binding: Some(openbitfun_core_types::model_pool::ModelPoolBinding {
            pool_id: "bound-pool".into(),
            model_id: "previous-member".into(),
            configuration_fingerprint: String::new(),
            generation: 0,
        }),
        ..Default::default()
    };
    for policy in [
        SessionModelBindingPolicy::Mutable,
        SessionModelBindingPolicy::ApprovedImmutable,
    ] {
        config.model_binding_policy = policy;
        assert_eq!(ConversationCoordinator::input_image_pool_id(&config), None);
    }
    config.model_binding_policy = SessionModelBindingPolicy::Pool;
    assert_eq!(
        ConversationCoordinator::input_image_pool_id(&config),
        Some("stale-pool")
    );
    config.model_pool_id = None;
    assert_eq!(
        ConversationCoordinator::input_image_pool_id(&config),
        Some("bound-pool")
    );
}

#[test]
fn input_image_count_admission_uses_the_resolved_native_model_limit() {
    use crate::service::config::types::{AIModelConfig, ModelCapability};

    let mut config = test_ai_config(vec![AIModelConfig {
        id: "limited-vision".into(),
        name: "Limited vision".into(),
        enabled: true,
        capabilities: vec![
            ModelCapability::TextChat,
            ModelCapability::ImageUnderstanding,
        ],
        max_images_per_request: Some(3),
        ..Default::default()
    }]);
    ConversationCoordinator::validate_input_image_count(&config, Some("limited-vision"), None, 3)
        .expect("the entire user batch fits");
    let error = ConversationCoordinator::validate_input_image_count(
        &config,
        Some("limited-vision"),
        None,
        4,
    )
    .expect_err("a user batch must not be silently truncated");
    assert!(matches!(error, OpenBitFunError::Validation(message) if message.contains("4 > 3")));

    config.models[0].max_images_per_request = None;
    ConversationCoordinator::validate_input_image_count(&config, Some("limited-vision"), None, 20)
        .expect("legacy models retain their existing behavior");
    config.models[0].max_images_per_request = Some(3);
    config.models[0].capabilities = vec![ModelCapability::TextChat];
    ConversationCoordinator::validate_input_image_count(&config, Some("limited-vision"), None, 4)
        .expect("text-only models receive analyze_image paths instead of a pixel batch");
}

fn image_admission_model(
    id: &str,
    limit: Option<usize>,
) -> crate::service::config::types::AIModelConfig {
    use crate::service::config::types::{AIModelConfig, ModelCapability};
    AIModelConfig {
        id: id.into(),
        name: id.into(),
        enabled: true,
        capabilities: vec![
            ModelCapability::TextChat,
            ModelCapability::ImageUnderstanding,
        ],
        max_images_per_request: limit,
        ..Default::default()
    }
}

#[test]
fn input_image_count_admission_uses_all_automatic_channels_but_only_the_manual_channel() {
    use openbitfun_config_contracts::channel_routing::ModelChannelRouting;
    let mut primary = image_admission_model("primary-vision", Some(5));
    primary.channel_routing = Some(ModelChannelRouting {
        fallback_model_ids: vec!["backup-vision".into(), "legacy-vision".into()],
        ..Default::default()
    });
    let mut config = test_ai_config(vec![
        primary,
        image_admission_model("backup-vision", Some(3)),
        image_admission_model("legacy-vision", None),
    ]);
    let error = ConversationCoordinator::validate_input_image_count(
        &config,
        Some("primary-vision"),
        None,
        5,
    )
    .expect_err("new user pixels must survive a later fallback");
    assert!(matches!(error, OpenBitFunError::Validation(message) if message.contains("5 > 3")));
    ConversationCoordinator::validate_input_image_count(&config, Some("primary-vision"), None, 3)
        .unwrap();

    config.models[0]
        .channel_routing
        .as_mut()
        .unwrap()
        .manual_channel = Some("primary-vision".into());
    ConversationCoordinator::validate_input_image_count(&config, Some("primary-vision"), None, 5)
        .expect("manual channels do not spill to the smaller backup");
    config.models[1].max_images_per_request = Some(6);
    config.models[0]
        .channel_routing
        .as_mut()
        .unwrap()
        .manual_channel = Some("backup-vision".into());
    ConversationCoordinator::validate_input_image_count(&config, Some("primary-vision"), None, 6)
        .expect("an unselected primary cannot restrict the manual backup");
}

#[test]
fn input_image_count_admission_uses_the_pool_and_member_channel_common_budget() {
    use crate::service::config::types::ModelCapability;
    use openbitfun_config_contracts::channel_routing::ModelChannelRouting;
    let mut disabled = image_admission_model("disabled", Some(1));
    disabled.enabled = false;
    disabled.channel_routing = Some(ModelChannelRouting {
        fallback_model_ids: vec!["unrelated-vision".into()],
        ..Default::default()
    });
    let mut text_only = image_admission_model("text-only", Some(1));
    text_only.capabilities = vec![ModelCapability::TextChat];
    let mut config = test_ai_config(vec![
        image_admission_model("large-vision", Some(5)),
        image_admission_model("small-vision", Some(3)),
        image_admission_model("legacy-vision", None),
        disabled,
        text_only,
        image_admission_model("unrelated-vision", Some(1)),
        image_admission_model("member-backup", Some(2)),
    ]);
    config.model_pools.pools.push(serde_json::json!({
            "id": "vision-pool", "name": "Vision pool", "strategy": "balanced",
            "member_model_ids": ["large-vision", "small-vision", "legacy-vision", "disabled", "text-only"]
        }));
    let error = ConversationCoordinator::validate_input_image_count(
        &config,
        Some("large-vision"),
        Some("vision-pool"),
        5,
    )
    .expect_err("the first selected member cannot widen the pool budget");
    assert!(matches!(error, OpenBitFunError::Validation(message) if message.contains("5 > 3")));
    ConversationCoordinator::validate_input_image_count(
        &config,
        Some("large-vision"),
        Some("vision-pool"),
        3,
    )
    .expect("disabled, unrelated, text-only and unspecified limits do not tighten the pool");
    ConversationCoordinator::validate_input_image_count(&config, None, Some("vision-pool"), 3)
        .expect("queue and steering admission need no current pool member");

    config.models[0].channel_routing = Some(ModelChannelRouting {
        fallback_model_ids: vec!["member-backup".into()],
        ..Default::default()
    });
    let error = ConversationCoordinator::validate_input_image_count(
        &config,
        Some("small-vision"),
        Some("vision-pool"),
        3,
    )
    .expect_err("every enabled member's fallback also participates in the common budget");
    assert!(matches!(error, OpenBitFunError::Validation(message) if message.contains("3 > 2")));
}
