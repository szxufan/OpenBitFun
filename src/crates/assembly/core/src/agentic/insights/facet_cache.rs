//! Disk cache for per-session facet extraction (fingerprint-invalidated).

use crate::agentic::insights::types::{SessionFacet, SessionTranscript};
use crate::infrastructure::get_path_manager_arc;
use crate::util::errors::OpenBitFunResult;
use log::debug;
use openbitfun_services_core::json_store::JsonFileStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;

const CACHE_SUBDIR: &str = "insights-facet-cache";

#[derive(Serialize, Deserialize)]
struct CachedFacetFile {
    fingerprint: String,
    facet: SessionFacet,
}

fn compute_fingerprint(
    transcript: &SessionTranscript,
    model_identity: &[&str],
    prompt: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(transcript.last_activity_unix_secs.to_be_bytes());
    hasher.update(transcript.turn_count.to_be_bytes());
    for part in [
        "insights-facet-v2",
        transcript
            .session_storage_scope
            .as_deref()
            .unwrap_or_default(),
        &transcript.session_id,
    ]
    .into_iter()
    .chain(model_identity.iter().copied())
    .chain(std::iter::once(prompt))
    {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn cache_file_path(transcript: &SessionTranscript) -> OpenBitFunResult<std::path::PathBuf> {
    let pm = get_path_manager_arc();
    let safe = transcript
        .session_id
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect::<String>();
    let mut workspace_hasher = Sha256::new();
    if let Some(workspace_path) = transcript.session_storage_scope.as_deref() {
        workspace_hasher.update(workspace_path.as_bytes());
    }
    let workspace_hash = format!("{:x}", workspace_hasher.finalize());
    Ok(pm
        .user_data_dir()
        .join(CACHE_SUBDIR)
        .join(format!("{}-{safe}.json", &workspace_hash[..16])))
}

pub async fn try_load_cached_facet(
    transcript: &SessionTranscript,
    model_identity: &[&str],
    prompt: &str,
) -> OpenBitFunResult<Option<SessionFacet>> {
    let path = match cache_file_path(transcript) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    let json = match fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let parsed: CachedFacetFile = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let want = compute_fingerprint(transcript, model_identity, prompt);
    if parsed.fingerprint != want {
        return Ok(None);
    }
    Ok(Some(parsed.facet))
}

pub async fn save_cached_facet(
    transcript: &SessionTranscript,
    facet: &SessionFacet,
    model_identity: &[&str],
    prompt: &str,
) -> OpenBitFunResult<()> {
    let path = cache_file_path(transcript)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let payload = CachedFacetFile {
        fingerprint: compute_fingerprint(transcript, model_identity, prompt),
        facet: facet.clone(),
    };
    JsonFileStore
        .write_atomic_strict(&path, &payload)
        .await
        .map_err(|error| crate::OpenBitFunError::io(error.to_string()))?;
    debug!("Saved facet cache {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(workspace_path: &str) -> SessionTranscript {
        SessionTranscript {
            session_id: "same-session".to_string(),
            agent_type: "Standard".to_string(),
            session_name: "Session".to_string(),
            session_storage_scope: Some(workspace_path.to_string()),
            last_activity_unix_secs: 10,
            duration_minutes: 1,
            message_count: 2,
            turn_count: 1,
            created_at: "2026-07-16T00:00:00Z".to_string(),
            transcript: "[User]: test".to_string(),
            tool_names: vec![],
            has_errors: false,
        }
    }

    #[test]
    fn changing_model_language_or_template_invalidates_the_fingerprint() {
        let transcript = transcript("workspace-a");
        let baseline = compute_fingerprint(&transcript, &["model-a"], "Analyze in English");
        for (model, prompt) in [
            ("model-b", "Analyze in English"),
            ("model-a", "Analyze in Chinese"),
            ("model-a", "Summarize in English"),
        ] {
            assert_ne!(baseline, compute_fingerprint(&transcript, &[model], prompt));
        }
    }

    #[test]
    fn fingerprint_is_scoped_by_workspace() {
        assert_ne!(
            compute_fingerprint(&transcript("workspace-a"), &["model"], "prompt"),
            compute_fingerprint(&transcript("workspace-b"), &["model"], "prompt")
        );
    }
}
