use super::super::SessionManagerConfig;
use super::support::{test_manager, test_manager_with_config, TestWorkspace};
use crate::agentic::core::SessionConfig;
use crate::agentic::persistence::PersistenceManager;
use crate::agentic::session::{
    PromptCachePolicy, PromptCacheScope, SystemPromptCacheIdentity, UserContextCacheIdentity,
};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn prompt_cache_persists_across_session_restore() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager(persistence_manager.clone());
    let workspace_path = workspace.path().to_string_lossy().to_string();
    let session = manager
        .create_session(
            "Prompt cache".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace_path),
                ..Default::default()
            },
        )
        .await
        .expect("session should be created");
    let identity = SystemPromptCacheIdentity::new("template:standard_mode");
    let user_context_identity =
        UserContextCacheIdentity::new("workspace_context|workspace_instructions|project_layout");

    manager
        .remember_system_prompt(
            &session.session_id,
            identity.clone(),
            "cached system prompt".to_string(),
        )
        .await;
    manager
        .remember_user_context(
            &session.session_id,
            user_context_identity.clone(),
            "cached user context".to_string(),
        )
        .await;

    manager.evict_loaded_session_for_test(&session.session_id);
    let restored_manager = test_manager(persistence_manager);
    restored_manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("session should restore");

    assert_eq!(
        restored_manager
            .cached_system_prompt(&session.session_id, &identity)
            .await,
        Some("cached system prompt".to_string())
    );
    assert_eq!(
        restored_manager
            .cached_user_context(&session.session_id, &user_context_identity)
            .await,
        Some("cached user context".to_string())
    );
}

#[tokio::test]
async fn prompt_cache_invalidation_removes_persisted_entries() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager(persistence_manager.clone());
    let workspace_path = workspace.path().to_string_lossy().to_string();
    let session = manager
        .create_session(
            "Prompt cache".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace_path),
                ..Default::default()
            },
        )
        .await
        .expect("session should be created");
    let identity = SystemPromptCacheIdentity::new("template:standard_mode");
    let user_context_identity =
        UserContextCacheIdentity::new("workspace_context|workspace_instructions|project_layout");

    manager
        .remember_system_prompt(
            &session.session_id,
            identity.clone(),
            "cached system prompt".to_string(),
        )
        .await;
    manager
        .remember_user_context(
            &session.session_id,
            user_context_identity.clone(),
            "cached user context".to_string(),
        )
        .await;

    manager
        .invalidate_prompt_cache(&session.session_id, PromptCacheScope::All, "test")
        .await;

    manager.evict_loaded_session_for_test(&session.session_id);
    let restored_manager = test_manager(persistence_manager.clone());
    restored_manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("session should restore");

    assert_eq!(
        restored_manager
            .cached_system_prompt(&session.session_id, &identity)
            .await,
        None
    );
    assert_eq!(
        restored_manager
            .cached_user_context(&session.session_id, &user_context_identity)
            .await,
        None
    );
    assert_eq!(
        persistence_manager
            .load_prompt_cache(workspace.path(), &session.session_id)
            .await
            .expect("prompt cache load should succeed"),
        None
    );
}

#[tokio::test]
async fn prompt_cache_invalidation_waits_for_an_inflight_lazy_restore() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = Arc::new(test_manager(persistence_manager.clone()));
    let session = manager
        .create_session(
            "Prompt cache".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should be created");
    let identity =
        UserContextCacheIdentity::new("workspace_context|workspace_instructions|project_layout");

    manager
        .remember_user_context(
            &session.session_id,
            identity.clone(),
            "persisted user context".to_string(),
        )
        .await;
    manager
        .prompt_cache_store
        .delete_session(&session.session_id);

    let operation_guard = manager
        .prompt_cache_operation_locks
        .lock(&session.session_id)
        .await;
    let lookup_manager = manager.clone();
    let lookup_session_id = session.session_id.clone();
    let lookup_identity = identity.clone();
    let lookup = tokio::spawn(async move {
        lookup_manager
            .cached_user_context(&lookup_session_id, &lookup_identity)
            .await
    });
    tokio::task::yield_now().await;
    let invalidate_manager = manager.clone();
    let invalidate_session_id = session.session_id.clone();
    let invalidate = tokio::spawn(async move {
        invalidate_manager
            .invalidate_prompt_cache(
                &invalidate_session_id,
                PromptCacheScope::UserContext,
                "test",
            )
            .await;
    });
    tokio::task::yield_now().await;
    drop(operation_guard);

    assert_eq!(
        lookup.await.expect("lookup task should complete"),
        Some("persisted user context".to_string())
    );
    invalidate.await.expect("invalidation task should complete");
    assert_eq!(
        persistence_manager
            .load_prompt_cache(workspace.path(), &session.session_id)
            .await
            .expect("prompt cache load should succeed"),
        None
    );
    manager
        .prompt_cache_store
        .delete_session(&session.session_id);
    assert_eq!(
        manager
            .cached_user_context(&session.session_id, &identity)
            .await,
        None
    );
}

#[tokio::test]
async fn clone_prompt_cache_copies_runtime_and_persisted_entries() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager(persistence_manager.clone());
    let workspace_path = workspace.path().to_string_lossy().to_string();
    let source_session = manager
        .create_session(
            "Prompt cache source".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace_path.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("source session should be created");
    let target_session = manager
        .create_session(
            "Prompt cache target".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace_path),
                ..Default::default()
            },
        )
        .await
        .expect("target session should be created");
    let identity = SystemPromptCacheIdentity::new("template:standard_mode");
    let user_context_identity =
        UserContextCacheIdentity::new("workspace_context|workspace_instructions|project_layout");

    manager
        .remember_system_prompt(
            &source_session.session_id,
            identity.clone(),
            "cached system prompt".to_string(),
        )
        .await;
    manager
        .remember_user_context(
            &source_session.session_id,
            user_context_identity.clone(),
            "cached user context".to_string(),
        )
        .await;

    assert!(
        manager
            .clone_prompt_cache(&source_session.session_id, &target_session.session_id)
            .await
    );
    assert_eq!(
        manager
            .cached_system_prompt(&target_session.session_id, &identity)
            .await,
        Some("cached system prompt".to_string())
    );
    assert_eq!(
        manager
            .cached_user_context(&target_session.session_id, &user_context_identity)
            .await,
        Some("cached user context".to_string())
    );
    assert_eq!(
        persistence_manager
            .load_prompt_cache(workspace.path(), &target_session.session_id)
            .await
            .expect("prompt cache load should succeed")
            .expect("cloned prompt cache should persist"),
        persistence_manager
            .load_prompt_cache(workspace.path(), &source_session.session_id)
            .await
            .expect("source prompt cache load should succeed")
            .expect("source prompt cache should exist")
    );
}

#[tokio::test]
async fn prompt_cache_persistence_ttl_only_affects_cold_start_restore() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager_with_config(
        persistence_manager.clone(),
        SessionManagerConfig {
            max_active_sessions: 100,
            session_idle_timeout: Duration::from_secs(3600),
            auto_save_interval: Duration::from_secs(300),
            enable_persistence: true,
            prompt_cache_policy: PromptCachePolicy {
                cache_ttl: None,
                persistence_ttl: Some(Duration::from_millis(0)),
            },
        },
    );
    let workspace_path = workspace.path().to_string_lossy().to_string();
    let session = manager
        .create_session(
            "Prompt cache".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace_path),
                ..Default::default()
            },
        )
        .await
        .expect("session should be created");
    let identity = SystemPromptCacheIdentity::new("template:standard_mode");
    let user_context_identity =
        UserContextCacheIdentity::new("workspace_context|workspace_instructions|project_layout");

    manager
        .remember_system_prompt(
            &session.session_id,
            identity.clone(),
            "cached system prompt".to_string(),
        )
        .await;
    manager
        .remember_user_context(
            &session.session_id,
            user_context_identity.clone(),
            "cached user context".to_string(),
        )
        .await;

    assert_eq!(
        manager
            .cached_system_prompt(&session.session_id, &identity)
            .await,
        Some("cached system prompt".to_string())
    );
    assert_eq!(
        manager
            .cached_user_context(&session.session_id, &user_context_identity)
            .await,
        Some("cached user context".to_string())
    );

    manager.evict_loaded_session_for_test(&session.session_id);
    let restored_manager = test_manager_with_config(
        persistence_manager.clone(),
        SessionManagerConfig {
            max_active_sessions: 100,
            session_idle_timeout: Duration::from_secs(3600),
            auto_save_interval: Duration::from_secs(300),
            enable_persistence: true,
            prompt_cache_policy: PromptCachePolicy {
                cache_ttl: None,
                persistence_ttl: Some(Duration::from_millis(0)),
            },
        },
    );
    restored_manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("session should restore");

    assert_eq!(
        restored_manager
            .cached_system_prompt(&session.session_id, &identity)
            .await,
        None
    );
    assert_eq!(
        restored_manager
            .cached_user_context(&session.session_id, &user_context_identity)
            .await,
        None
    );
}
