//! SSH transport for persistent OpenBitFun dispatch jobs.
//!
//! The target-side runner is the `openbitfun dispatch` CLI surface. This module is
//! deliberately only a controller transport: the remote CLI owns jobs,
//! workspaces, sessions, transcripts, process detachment, supervision, and
//! cancellation semantics. Submission probes the existing target CLI before workspace delivery.
//!

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;

use super::manager::SSHConnectionManager;
use super::product_paths::product_data_path;
use super::remote_git::shell_quote_posix;
use super::types::SSHCommandOptions;

const COMMAND_TIMEOUT_MS: u64 = 30_000;
const WORKSPACE_OPERATION_WAIT: Duration = Duration::from_secs(30 * 60);
const WORKSPACE_OPERATION_POLL_INTERVAL: Duration = Duration::from_millis(750);
/// A result bundle carries only commits since the dispatch baseline, so it is
/// bounded well below a full repository clone in the usual case.
const MAX_RESULT_BUNDLE_BYTES: u64 = 1024 * 1024 * 1024;
const RESULT_BUNDLE_CHUNK_BYTES: u64 = 256 * 1024;
const MAX_RESULT_BUNDLE_CHUNK_BASE64_BYTES: usize = 384 * 1024;

struct UnverifiedResultBundle {
    path: std::path::PathBuf,
    verified: bool,
}

impl UnverifiedResultBundle {
    fn new(path: &std::path::Path) -> Self {
        Self {
            path: path.to_path_buf(),
            verified: false,
        }
    }

    fn retain(&mut self) {
        self.verified = true;
    }
}

impl Drop for UnverifiedResultBundle {
    fn drop(&mut self) {
        if !self.verified {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
const DISPATCH_PROTOCOL_VERSION: u64 =
    openbitfun_services_core::dispatch_contract::DISPATCH_PROTOCOL_VERSION as u64;
/// Derived once from the shared contract: the unconditional target surface
/// plus the platform-conditional detached worker.
static REQUIRED_DISPATCH_CAPABILITIES: std::sync::LazyLock<Vec<&'static str>> =
    std::sync::LazyLock::new(|| {
        openbitfun_services_core::dispatch_contract::dispatch_required_target_capabilities()
            .collect()
    });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchCliRelease {
    pub version: String,
    pub target: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchSshProbe {
    pub cli_installed: bool,
    pub cli_path: Option<String>,
    pub os: String,
    pub arch: String,
    pub install_supported: bool,
    pub install_error: Option<String>,
    pub protocol_error: Option<String>,
    pub release: Option<DispatchCliRelease>,
    pub protocol: Option<Value>,
    /// Legacy probe field retained for older controllers.
    pub prebuilt_incompatible: Option<String>,
}

#[derive(Debug)]
struct RemoteTarget {
    os: String,
    arch: String,
    home: String,
    cli_path: Option<String>,
}

/// Inspect the target platform and the capabilities of its installed CLI.
pub async fn probe(
    manager: &SSHConnectionManager,
    connection_id: &str,
    workspace_path: Option<&str>,
) -> Result<DispatchSshProbe> {
    ensure_plain_ssh_target(manager, connection_id).await?;
    let target = probe_remote_target(manager, connection_id).await?;

    let mut protocol = None;
    let mut protocol_error = None;
    if let Some(cli_path) = target.cli_path.as_deref() {
        let request = workspace_path
            .map(|path| serde_json::json!({ "workspacePath": path }))
            .unwrap_or_else(|| serde_json::json!({}));
        match invoke_json_at_path(
            manager,
            connection_id,
            &target.home,
            cli_path,
            "probe",
            &request,
        )
        .await
        {
            Ok(response) => {
                protocol_error = validate_dispatch_protocol(&response, None)
                    .err()
                    .map(|error| error.to_string());
                protocol = Some(response);
            }
            Err(error) => protocol_error = Some(error.to_string()),
        }
    }

    Ok(DispatchSshProbe {
        cli_installed: target.cli_path.is_some(),
        cli_path: target.cli_path,
        os: target.os,
        arch: target.arch,
        install_supported: false,
        install_error: None,
        protocol_error,
        release: None,
        protocol,
        prebuilt_incompatible: None,
    })
}

#[cfg(test)]
fn dispatch_protocol_is_compatible(protocol: &Value) -> bool {
    validate_dispatch_protocol(protocol, None).is_ok()
}

/// Validate the target-side protocol immediately before submission.
///
/// `approval_policy = None` is used by the readiness probe and requires the
/// complete dispatch surface. Submission may validate only the selected
/// unattended approval behavior in addition to the transport invariants.
pub fn validate_dispatch_protocol(protocol: &Value, approval_policy: Option<&str>) -> Result<()> {
    let expected_product_id = openbitfun_services_core::product_identity::product_id();
    let product_id = protocol
        .get("productId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("dispatch target returned no productId"))?;
    if product_id != expected_product_id {
        return Err(anyhow!(
            "dispatch product is incompatible; expected {expected_product_id}, received {product_id}"
        ));
    }

    let expected_data_namespace = openbitfun_services_core::product_identity::data_namespace();
    let data_namespace = protocol
        .get("dataNamespace")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("dispatch target returned no dataNamespace"))?;
    if data_namespace != expected_data_namespace {
        return Err(anyhow!(
            "dispatch data namespace is incompatible; expected {expected_data_namespace}, received {data_namespace}"
        ));
    }

    if protocol.get("protocolVersion").and_then(Value::as_u64) != Some(DISPATCH_PROTOCOL_VERSION) {
        return Err(anyhow!(
            "dispatch protocol version is incompatible; expected {}",
            DISPATCH_PROTOCOL_VERSION
        ));
    }
    let Some(capabilities) = protocol.get("capabilities").and_then(Value::as_array) else {
        return Err(anyhow!("dispatch target returned no capability list"));
    };
    // One source list: submission narrows the approval_* entries to the
    // selected policy; probing (None) requires the complete surface.
    let mut required: Vec<&'static str> = REQUIRED_DISPATCH_CAPABILITIES
        .iter()
        .copied()
        .filter(|capability| !capability.starts_with("approval_"))
        .collect();
    match approval_policy {
        Some("auto") => required.push("approval_auto"),
        Some("reject-and-report") => required.push("approval_reject_and_report"),
        Some("remote") => required.push("approval_remote"),
        Some(_) => return Err(anyhow!("unsupported dispatch approval policy")),
        None => required.extend(
            REQUIRED_DISPATCH_CAPABILITIES
                .iter()
                .copied()
                .filter(|capability| capability.starts_with("approval_")),
        ),
    }
    let missing = required
        .iter()
        .copied()
        .filter(|required| {
            !capabilities
                .iter()
                .any(|capability| capability.as_str() == Some(*required))
        })
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(anyhow!(
            "dispatch target is missing required capabilities: {}",
            missing.join(", ")
        ));
    }
    Ok(())
}

/// Keys of the `ai` config section that make up "model configuration": the
/// model catalog (credentials included) plus every default-selection table the
/// target consults when resolving a ready model.
const MODEL_CONFIG_KEYS: [&str; 5] = [
    "models",
    "default_models",
    "agent_model_defaults",
    "agent_model_selections",
    "model_pools",
];

/// Write the controller's model configuration into the target's global config
/// so `openbitfun dispatch probe` can report a ready model.
///
/// `ai_model_config` is the snake_case `ai` slice restricted to
/// [`MODEL_CONFIG_KEYS`], exactly as `app.json` stores it. Everything else in
/// an existing target `app.json` is preserved; a target file that exists but
/// cannot be read or parsed aborts the sync instead of being overwritten. The
/// write is atomic (temp file + rename) and owner-only, because model entries
/// carry API credentials.
pub async fn sync_model_config(
    manager: &SSHConnectionManager,
    connection_id: &str,
    ai_model_config: &Value,
) -> Result<()> {
    let payload = validate_model_config_payload(ai_model_config)?;
    ensure_plain_ssh_target(manager, connection_id).await?;

    let locate = exec_lines(manager, connection_id, locate_target_config_script()).await?;
    let get = |key: &str| {
        locate
            .lines()
            .find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix('='))
            })
            .unwrap_or("")
            .trim()
            .to_string()
    };
    if get("os") == "unsupported" {
        return Err(anyhow!(
            "model configuration sync supports only Linux and macOS SSH targets"
        ));
    }
    let config_dir = get("dir");
    if config_dir.is_empty() {
        return Err(anyhow!(
            "could not resolve the target OpenBitFun config directory"
        ));
    }
    let config_path = format!("{config_dir}/app.json");

    let existing = if get("config") == "1" {
        let bytes = manager
            .sftp_read(connection_id, &config_path)
            .await
            .context("read existing target app.json; refusing to overwrite it blindly")?;
        Some(String::from_utf8(bytes).context("target app.json is not UTF-8")?)
    } else {
        None
    };
    let merged = merge_model_config(existing.as_deref(), payload)?;

    exec_ok(
        manager,
        connection_id,
        &format!(
            "mkdir -p {dir} && chmod 700 \"$(dirname {dir})\" {dir}",
            dir = shell_quote_posix(&config_dir),
        ),
    )
    .await?;
    let data_namespace = openbitfun_services_core::product_identity::data_namespace();
    let staging_path = format!("{config_path}.{data_namespace}-sync.tmp");
    manager
        .sftp_write(connection_id, &staging_path, merged.as_bytes())
        .await
        .context("stage merged target app.json")?;
    exec_ok(
        manager,
        connection_id,
        &format!(
            "chmod 600 {staged} && mv -f {staged} {config}",
            staged = shell_quote_posix(&staging_path),
            config = shell_quote_posix(&config_path),
        ),
    )
    .await
}

fn validate_model_config_payload(
    ai_model_config: &Value,
) -> Result<&serde_json::Map<String, Value>> {
    let payload = ai_model_config
        .as_object()
        .ok_or_else(|| anyhow!("model configuration payload must be a JSON object"))?;
    if let Some(unexpected) = payload
        .keys()
        .find(|key| !MODEL_CONFIG_KEYS.contains(&key.as_str()))
    {
        return Err(anyhow!(
            "model configuration payload has unexpected key '{unexpected}'"
        ));
    }
    if payload
        .get("models")
        .and_then(Value::as_array)
        .is_none_or(|models| models.is_empty())
    {
        return Err(anyhow!(
            "the controller has no configured AI models to sync"
        ));
    }
    Ok(payload)
}

/// Graft the model-configuration keys onto an existing target config document,
/// leaving every other setting untouched.
fn merge_model_config(
    existing: Option<&str>,
    payload: &serde_json::Map<String, Value>,
) -> Result<String> {
    let mut root = match existing.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => serde_json::from_str::<Value>(text)
            .context("target app.json exists but is not valid JSON; refusing to overwrite it")?,
        None => Value::Object(serde_json::Map::new()),
    };
    let root_map = root
        .as_object_mut()
        .ok_or_else(|| anyhow!("target app.json is not a JSON object; refusing to overwrite it"))?;
    let ai = root_map
        .entry("ai")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let ai_map = ai.as_object_mut().ok_or_else(|| {
        anyhow!("target app.json has a non-object `ai` section; refusing to overwrite it")
    })?;
    for (key, value) in payload {
        ai_map.insert(key.clone(), value.clone());
    }
    serde_json::to_string_pretty(&root).context("encode merged target app.json")
}

/// Where the target CLI reads its global config from, mirroring the
/// `dirs::config_dir()` resolution inside the CLI itself.
fn locate_target_config_script() -> &'static str {
    r#"
LC_ALL=C
case "$(uname -s 2>/dev/null)" in
  Darwin) CONFIG_DIR="$HOME/Library/Application Support/openbitfun/config" ;;
  Linux) CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/openbitfun/config" ;;
  *) printf 'os=unsupported\n'; exit 0 ;;
esac
printf 'os=supported\n'
printf 'dir=%s\n' "$CONFIG_DIR"
if [ -f "$CONFIG_DIR/app.json" ]; then printf 'config=1\n'; else printf 'config=0\n'; fi
"#
}

async fn exec_lines(
    manager: &SSHConnectionManager,
    connection_id: &str,
    script: &str,
) -> Result<String> {
    let result = manager
        .execute_command_with_options(
            connection_id,
            script,
            SSHCommandOptions {
                timeout_ms: Some(COMMAND_TIMEOUT_MS),
                cancellation_token: None,
            },
        )
        .await?;
    ensure_command_completed(&result, "inspect SSH dispatch target")?;
    if result.exit_code != 0 {
        return Err(remote_command_error(
            "inspect SSH dispatch target",
            result.exit_code,
            &result.stdout,
            &result.stderr,
        ));
    }
    Ok(result.stdout)
}

pub async fn submit(
    manager: &SSHConnectionManager,
    connection_id: &str,
    request: &Value,
) -> Result<Value> {
    invoke_json(manager, connection_id, "submit", request).await
}

pub async fn status(
    manager: &SSHConnectionManager,
    connection_id: &str,
    request: &Value,
) -> Result<Value> {
    invoke_json(manager, connection_id, "status", request).await
}

pub async fn cancel(
    manager: &SSHConnectionManager,
    connection_id: &str,
    request: &Value,
) -> Result<Value> {
    invoke_json(manager, connection_id, "cancel", request).await
}

pub async fn list(
    manager: &SSHConnectionManager,
    connection_id: &str,
    request: &Value,
) -> Result<Value> {
    invoke_json(manager, connection_id, "list", request).await
}

pub async fn answer(
    manager: &SSHConnectionManager,
    connection_id: &str,
    request: &Value,
) -> Result<Value> {
    invoke_json(manager, connection_id, "answer", request).await
}

pub async fn append(
    manager: &SSHConnectionManager,
    connection_id: &str,
    request: &Value,
) -> Result<Value> {
    invoke_json(manager, connection_id, "append", request).await
}

/// Start the next turn of a dispatch session that has already finished one.
pub async fn continue_job(
    manager: &SSHConnectionManager,
    connection_id: &str,
    request: &Value,
) -> Result<Value> {
    invoke_json(manager, connection_id, "continue", request).await
}

pub async fn query(
    manager: &SSHConnectionManager,
    connection_id: &str,
    request: &Value,
) -> Result<Value> {
    invoke_json(manager, connection_id, "query", request).await
}

/// Commit the target's worktree and fetch the Git bundle it produced.
///
/// Downloads only. The controller decides separately whether to fast-forward
/// its baseline worktree onto the fetched branch, so nothing in the user's
/// repository moves as a side effect of asking.
pub async fn sync_workspace(
    manager: &SSHConnectionManager,
    connection_id: &str,
    job_id: &str,
    message: Option<&str>,
    known_head: Option<&str>,
    destination: &std::path::Path,
) -> Result<Value> {
    ensure_plain_ssh_target(manager, connection_id).await?;
    let target = probe_remote_target(manager, connection_id).await?;
    let cli_path = target.cli_path.as_deref().ok_or_else(|| {
        anyhow!("OpenBitFun CLI is not installed on the SSH target; install it before syncing")
    })?;

    // A clean incremental sync has `headCommit == knownHead`. Without an
    // invocation identity, the target cannot distinguish this call's poll
    // from a later click that intentionally checks for newer work, and would
    // restart the completed no-op forever.
    let mut request = serde_json::json!({
        "jobId": job_id,
        "operationId": uuid::Uuid::new_v4().as_simple().to_string(),
    });
    if let Some(message) = message.map(str::trim).filter(|value| !value.is_empty()) {
        request["message"] = Value::String(message.to_string());
    }
    if let Some(head) = known_head.map(str::trim).filter(|value| !value.is_empty()) {
        request["knownHead"] = Value::String(head.to_string());
    }
    let deadline = tokio::time::Instant::now() + WORKSPACE_OPERATION_WAIT;
    let response = loop {
        let response = invoke_json_at_path(
            manager,
            connection_id,
            &target.home,
            cli_path,
            "__workspace_sync",
            &request,
        )
        .await?;
        if response.get("pending").and_then(Value::as_bool) != Some(true) {
            break response;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "Git workspace sync did not finish within {} minutes",
                WORKSPACE_OPERATION_WAIT.as_secs() / 60
            ));
        }
        tokio::time::sleep(WORKSPACE_OPERATION_POLL_INTERVAL).await;
    };

    if response.get("changed").and_then(Value::as_bool) != Some(true) {
        return Ok(response);
    }

    let bundle_path = response
        .get("bundlePath")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("dispatch target returned no result bundle path"))?;
    // The path comes from the target, so bound it to the managed job directory
    // before reading it, exactly as the upload path is bounded.
    validate_managed_result_path(&target.home, job_id, bundle_path)?;
    let expected_size = response
        .get("bundleSize")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("dispatch target returned no result bundle size"))?;
    if expected_size == 0 || expected_size > MAX_RESULT_BUNDLE_BYTES {
        return Err(anyhow!(
            "dispatch result bundle exceeds the {} MB safety limit",
            MAX_RESULT_BUNDLE_BYTES / (1024 * 1024)
        ));
    }
    let expected_digest = response
        .get("bundleSha256")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("dispatch target returned no result bundle digest"))?;

    // The bundle carries repository history. The outbound root is already
    // owner-only, but harden this level too rather than relying on a parent.
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create result staging {}", parent.display()))?;
        harden_result_directory(parent)?;
    }
    let mut staged_bundle = UnverifiedResultBundle::new(destination);
    write_private_file(destination, &[])?;
    let mut output = std::fs::OpenOptions::new()
        .append(true)
        .open(destination)
        .with_context(|| format!("open result staging {}", destination.display()))?;
    let mut digest = Sha256::new();
    let mut received = 0_u64;
    while received < expected_size {
        let chunk = invoke_json_at_path(
            manager,
            connection_id,
            &target.home,
            cli_path,
            "__workspace_sync_chunk",
            &serde_json::json!({
                "jobId": job_id,
                "offset": received,
                "length": RESULT_BUNDLE_CHUNK_BYTES,
            }),
        )
        .await?;
        let encoded = chunk
            .get("dataBase64")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("dispatch target returned no result chunk data"))?;
        if encoded.len() > MAX_RESULT_BUNDLE_CHUNK_BASE64_BYTES {
            return Err(anyhow!(
                "dispatch target returned an oversized result chunk"
            ));
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .context("decode dispatch result chunk")?;
        if decoded.is_empty() || decoded.len() as u64 > RESULT_BUNDLE_CHUNK_BYTES {
            return Err(anyhow!(
                "dispatch result bundle ended at {received} of {expected_size} bytes"
            ));
        }
        let next_offset = received.saturating_add(decoded.len() as u64);
        if next_offset > expected_size {
            return Err(anyhow!(
                "dispatch target returned more result bytes than it declared"
            ));
        }
        if chunk.get("offset").and_then(Value::as_u64) != Some(next_offset) {
            return Err(anyhow!(
                "dispatch target returned a mismatched result chunk offset"
            ));
        }
        std::io::Write::write_all(&mut output, &decoded)
            .with_context(|| format!("write result staging {}", destination.display()))?;
        digest.update(&decoded);
        received = next_offset;
        let eof = chunk.get("eof").and_then(Value::as_bool) == Some(true);
        if eof != (received == expected_size) {
            return Err(anyhow!(
                "dispatch target returned an inconsistent result end marker"
            ));
        }
    }
    output
        .sync_all()
        .with_context(|| format!("flush result staging {}", destination.display()))?;
    let actual_digest = format!("{:x}", digest.finalize());
    if !actual_digest.eq_ignore_ascii_case(expected_digest) {
        return Err(anyhow!("dispatch result bundle SHA-256 mismatch"));
    }
    staged_bundle.retain();

    let mut response = response;
    if let Some(object) = response.as_object_mut() {
        object.insert(
            "localBundlePath".to_string(),
            Value::String(destination.to_string_lossy().to_string()),
        );
    }
    Ok(response)
}

pub fn harden_result_directory(path: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restrict result staging {}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Create owner-only before writing, so the contents are never briefly governed
/// by the process umask.
pub fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    std::io::Write::write_all(&mut file, bytes)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// A result bundle may only be read from the managed directory of the job it
/// belongs to.
fn validate_managed_result_path(home: &str, job_id: &str, bundle_path: &str) -> Result<()> {
    let expected = product_data_path(home, &["dispatch", "workspaces", job_id, "result.bundle"]);
    if bundle_path != expected {
        return Err(anyhow!(
            "dispatch target returned an unexpected result bundle path"
        ));
    }
    Ok(())
}

/// The upload path for a delivered base bundle, bounded to the job directory.
fn validate_managed_bundle_upload_path(home: &str, job_id: &str, upload_path: &str) -> Result<()> {
    let expected = product_data_path(home, &["dispatch", "workspaces", job_id, "incoming.bundle"]);
    if upload_path != expected {
        return Err(anyhow!(
            "dispatch target returned an invalid managed bundle upload path"
        ));
    }
    Ok(())
}

/// Ask the target to check out this dispatch's baseline commit.
///
/// Returns the target's raw response so the controller can react to
/// `needsBundle` — the target is the only side that knows what its own clone
/// can reach, so the decision to ship objects belongs to it, not to a guess
/// made from this machine's remote-tracking refs.
pub async fn provision_workspace(
    manager: &SSHConnectionManager,
    connection_id: &str,
    request: &Value,
) -> Result<Value> {
    invoke_workspace_operation(
        manager,
        connection_id,
        "__workspace_provision",
        request,
        "Git workspace provisioning",
    )
    .await
}

/// Poll an idempotent target verb whose expensive Git work runs in a detached
/// CLI child. Every SSH command returns quickly, so losing one channel cannot
/// kill clone/fetch/bundle work that the next poll can observe.
async fn invoke_workspace_operation(
    manager: &SSHConnectionManager,
    connection_id: &str,
    verb: &'static str,
    request: &Value,
    operation: &str,
) -> Result<Value> {
    ensure_plain_ssh_target(manager, connection_id).await?;
    let target = probe_remote_target(manager, connection_id).await?;
    let cli_path = target.cli_path.as_deref().ok_or_else(|| {
        anyhow!("OpenBitFun CLI is not installed on the SSH target; install it before dispatching")
    })?;
    let deadline = tokio::time::Instant::now() + WORKSPACE_OPERATION_WAIT;
    loop {
        let response = invoke_json_at_path(
            manager,
            connection_id,
            &target.home,
            cli_path,
            verb,
            request,
        )
        .await?;
        if response.get("pending").and_then(Value::as_bool) != Some(true) {
            return Ok(response);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "{operation} did not finish within {} minutes",
                WORKSPACE_OPERATION_WAIT.as_secs() / 60
            ));
        }
        tokio::time::sleep(WORKSPACE_OPERATION_POLL_INTERVAL).await;
    }
}

/// Upload a Git bundle carrying the objects the target reported missing.
///
/// The target CLI chooses the owner-only upload path. This adapter validates
/// that the returned path stays under the target's managed dispatch root before
/// allowing SFTP to write it.
pub async fn upload_bundle(
    manager: &SSHConnectionManager,
    connection_id: &str,
    job_id: &str,
    sha256: &str,
    size: u64,
    bundle_path: &std::path::Path,
) -> Result<Value> {
    ensure_plain_ssh_target(manager, connection_id).await?;
    let target = probe_remote_target(manager, connection_id).await?;
    let cli_path = target.cli_path.as_deref().ok_or_else(|| {
        anyhow!("OpenBitFun CLI is not installed on the SSH target; install it before dispatching")
    })?;

    let begin = invoke_json_at_path(
        manager,
        connection_id,
        &target.home,
        cli_path,
        "__workspace_bundle_begin",
        &serde_json::json!({
            "protocolVersion": DISPATCH_PROTOCOL_VERSION,
            "jobId": job_id,
            "sha256": sha256,
            "size": size,
        }),
    )
    .await?;
    if begin
        .get("committed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(begin);
    }
    if begin.get("accepted").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!("dispatch target did not accept the bundle upload"));
    }

    let upload_path = product_data_path(
        &target.home,
        &["dispatch", "workspaces", job_id, "incoming.bundle"],
    );
    validate_managed_bundle_upload_path(&target.home, job_id, &upload_path)?;
    let local_size = std::fs::symlink_metadata(bundle_path)
        .with_context(|| format!("inspect dispatch bundle {}", bundle_path.display()))?
        .len();
    if local_size != size {
        return Err(anyhow!(
            "dispatch bundle changed before SSH upload: expected {size} bytes, found {local_size}"
        ));
    }
    let retained_offset = begin
        .get("offset")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("dispatch target returned no bundle upload offset"))?;
    if retained_offset > size {
        return Err(anyhow!(
            "dispatch target returned an invalid bundle upload offset"
        ));
    }
    if retained_offset < size {
        let written = manager
            .sftp_write_from_file(connection_id, &upload_path, bundle_path, size)
            .await
            .context("upload dispatch bundle over SFTP")?;
        if written != size {
            return Err(anyhow!(
                "dispatch bundle SFTP upload ended at {written} of {size} bytes"
            ));
        }
    }

    let commit_request = serde_json::json!({ "jobId": job_id });
    let deadline = tokio::time::Instant::now() + WORKSPACE_OPERATION_WAIT;
    loop {
        let response = invoke_json_at_path(
            manager,
            connection_id,
            &target.home,
            cli_path,
            "__workspace_bundle_commit",
            &commit_request,
        )
        .await?;
        if response.get("pending").and_then(Value::as_bool) != Some(true) {
            if response.get("committed").and_then(Value::as_bool) == Some(true) {
                return Ok(response);
            }
            return Err(anyhow!(
                "dispatch target did not commit the delivered bundle"
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "Git bundle import did not finish within {} minutes",
                WORKSPACE_OPERATION_WAIT.as_secs() / 60
            ));
        }
        tokio::time::sleep(WORKSPACE_OPERATION_POLL_INTERVAL).await;
    }
}

async fn invoke_json(
    manager: &SSHConnectionManager,
    connection_id: &str,
    verb: &'static str,
    request: &Value,
) -> Result<Value> {
    ensure_plain_ssh_target(manager, connection_id).await?;
    let target = probe_remote_target(manager, connection_id).await?;
    let cli_path = target.cli_path.as_deref().ok_or_else(|| {
        anyhow!("OpenBitFun CLI is not installed on the SSH target; confirm installation first")
    })?;
    invoke_json_at_path(
        manager,
        connection_id,
        &target.home,
        cli_path,
        verb,
        request,
    )
    .await
}

/// Transfer the request as an owner-only file instead of embedding its JSON in
/// the SSH command. Prompts can contain credentials or private source context;
/// command arguments would expose them to process listings and manager debug
/// previews on both machines.
async fn invoke_json_at_path(
    manager: &SSHConnectionManager,
    connection_id: &str,
    home: &str,
    cli_path: &str,
    verb: &'static str,
    request: &Value,
) -> Result<Value> {
    use openbitfun_core_types::agent_identity_wire::{
        translate_agent_identity_command, translate_agent_identity_response, AgentIdentityDialect,
    };
    let request =
        translate_agent_identity_command(verb, request.clone(), AgentIdentityDialect::Legacy)
            .map_err(anyhow::Error::msg)?;
    let request_dir = product_data_path(home, &["dispatch", "requests"]);
    exec_ok(
        manager,
        connection_id,
        &format!(
            "mkdir -p {dir} && chmod 700 {root} {dispatch} {dir}",
            root = shell_quote_posix(&product_data_path(home, &[])),
            dispatch = shell_quote_posix(&product_data_path(home, &["dispatch"])),
            dir = shell_quote_posix(&request_dir)
        ),
    )
    .await?;
    let request_path = format!("{request_dir}/{}.json", uuid::Uuid::new_v4().as_simple());
    let request_bytes = serde_json::to_vec(&request).context("serialize dispatch request")?;
    // Pre-create with 0600 before SFTP opens it. Creating first and chmodding
    // afterwards would leave a prompt briefly governed by the server's umask
    // (commonly 0644).
    exec_ok(
        manager,
        connection_id,
        &format!(
            "umask 077; : > {request}; chmod 600 {request}",
            request = shell_quote_posix(&request_path)
        ),
    )
    .await?;
    if let Err(error) = manager
        .sftp_write(connection_id, &request_path, &request_bytes)
        .await
        .context("stage dispatch request")
    {
        let _ = manager.sftp_remove(connection_id, &request_path).await;
        return Err(error);
    }

    let command = dispatch_command(cli_path, verb, &request_path);
    let result = manager
        .execute_command_with_options(
            connection_id,
            &command,
            SSHCommandOptions {
                timeout_ms: Some(COMMAND_TIMEOUT_MS),
                cancellation_token: None,
            },
        )
        .await;
    // The remote EXIT trap normally removes it. This covers channel-open and
    // transport failures before the shell installed that trap.
    let _ = manager.sftp_remove(connection_id, &request_path).await;
    let result = result?;
    ensure_command_completed(&result, &format!("dispatch {verb}"))?;
    if result.exit_code != 0 {
        return Err(remote_command_error(
            &format!("dispatch {verb}"),
            result.exit_code,
            &result.stdout,
            &result.stderr,
        ));
    }
    let response = serde_json::from_str(result.stdout.trim()).with_context(|| {
        format!(
            "dispatch {verb} returned invalid JSON: {}",
            bounded_detail(&result.stdout)
        )
    })?;
    translate_agent_identity_response(
        verb,
        response,
        &Value::Null,
        AgentIdentityDialect::Canonical,
    )
    .map_err(anyhow::Error::msg)
}

fn dispatch_command(cli_path: &str, verb: &str, request_path: &str) -> String {
    let cli = shell_quote_posix(cli_path);
    let request = shell_quote_posix(request_path);
    let verb = shell_quote_posix(verb);
    format!(
        "request={request}; \
         cleanup() {{ rm -f \"$request\"; }}; \
         trap cleanup EXIT; \
         trap 'exit 130' HUP INT TERM; \
         {cli} dispatch {verb} < \"$request\""
    )
}

async fn ensure_plain_ssh_target(
    manager: &SSHConnectionManager,
    connection_id: &str,
) -> Result<()> {
    let active_container = manager
        .get_connection_config(connection_id)
        .await
        .is_some_and(|config| config.container.is_some());
    let saved_container = manager
        .get_saved_connections()
        .await
        .into_iter()
        .find(|config| config.id == connection_id)
        .is_some_and(|config| config.container.is_some());
    if active_container || saved_container {
        return Err(anyhow!(
            "SSH dispatch does not support Docker-container connection targets"
        ));
    }
    Ok(())
}

async fn probe_remote_target(
    manager: &SSHConnectionManager,
    connection_id: &str,
) -> Result<RemoteTarget> {
    let script = probe_remote_target_script();
    let result = manager
        .execute_command_with_options(
            connection_id,
            script,
            SSHCommandOptions {
                timeout_ms: Some(COMMAND_TIMEOUT_MS),
                cancellation_token: None,
            },
        )
        .await?;
    ensure_command_completed(&result, "probe SSH dispatch target")?;
    if result.exit_code != 0 {
        return Err(remote_command_error(
            "probe SSH dispatch target",
            result.exit_code,
            &result.stdout,
            &result.stderr,
        ));
    }
    let get = |key: &str| {
        result
            .stdout
            .lines()
            .find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix('='))
            })
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let home = get("home");
    if home.is_empty() {
        return Err(anyhow!("could not resolve remote $HOME"));
    }
    let cli_path = get("cli");
    Ok(RemoteTarget {
        os: get("os"),
        arch: get("arch"),
        home,
        cli_path: (!cli_path.is_empty()).then_some(cli_path),
    })
}

fn probe_remote_target_script() -> &'static str {
    r#"
LC_ALL=C
printf 'os=%s\n' "$(uname -s 2>/dev/null || true)"
printf 'arch=%s\n' "$(uname -m 2>/dev/null || true)"
printf 'home=%s\n' "$HOME"
if [ -x "$HOME/.local/bin/openbitfun" ]; then
  OPENBITFUN_BIN="$HOME/.local/bin/openbitfun"
else
  OPENBITFUN_BIN="$(command -v openbitfun 2>/dev/null || true)"
fi
printf 'cli=%s\n' "$OPENBITFUN_BIN"
"#
}

async fn exec_ok(manager: &SSHConnectionManager, connection_id: &str, command: &str) -> Result<()> {
    let result = manager
        .execute_command_with_options(
            connection_id,
            command,
            SSHCommandOptions {
                timeout_ms: Some(COMMAND_TIMEOUT_MS),
                cancellation_token: None,
            },
        )
        .await?;
    ensure_command_completed(&result, "remote setup")?;
    if result.exit_code != 0 {
        return Err(remote_command_error(
            "remote setup",
            result.exit_code,
            &result.stdout,
            &result.stderr,
        ));
    }
    Ok(())
}

fn ensure_command_completed(
    result: &super::types::SSHCommandResult,
    operation: &str,
) -> Result<()> {
    if result.timed_out {
        return Err(anyhow!("{operation} timed out"));
    }
    if result.interrupted {
        return Err(anyhow!("{operation} was cancelled"));
    }
    Ok(())
}

fn remote_command_error(
    operation: &str,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) -> anyhow::Error {
    let detail = if stderr.trim().is_empty() {
        bounded_detail(stdout)
    } else {
        bounded_detail(stderr)
    };
    anyhow!("{operation} failed (exit {exit_code}): {detail}")
}

fn bounded_detail(value: &str) -> String {
    value.trim().chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_worktree_delivery_capabilities_are_required() {
        for capability in [
            "workspace_git_worktree",
            "workspace_git_bundle_upload",
            "workspace_git_sync",
        ] {
            assert!(
                REQUIRED_DISPATCH_CAPABILITIES.contains(&capability),
                "{capability} must fail closed because snapshot delivery no longer exists"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn staged_result_bundles_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temp dir");
        let staging = temp.path().join(".results");
        std::fs::create_dir_all(&staging).expect("staging");
        // Simulate a permissive umask having created it.
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).expect("loosen");
        harden_result_directory(&staging).expect("harden");
        assert_eq!(
            std::fs::metadata(&staging)
                .expect("stat")
                .permissions()
                .mode()
                & 0o777,
            0o700,
            "the staging directory holds user source and must not be world-readable"
        );

        let bundle = staging.join("job-1.tar.gz");
        write_private_file(&bundle, b"bundle bytes").expect("write");
        assert_eq!(
            std::fs::metadata(&bundle)
                .expect("stat")
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "the bundle itself must be owner-only"
        );
        assert_eq!(std::fs::read(&bundle).expect("read"), b"bundle bytes");

        // Rewriting must not widen the mode or leave a stale tail.
        write_private_file(&bundle, b"short").expect("rewrite");
        assert_eq!(
            std::fs::metadata(&bundle)
                .expect("stat")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(&bundle).expect("read"), b"short");
    }

    #[test]
    fn unverified_result_bundles_are_removed_but_verified_ones_are_retained() {
        let temp = tempfile::tempdir().expect("temp dir");
        let rejected = temp.path().join("rejected.bundle");
        std::fs::write(&rejected, b"tampered").expect("write rejected bundle");
        drop(UnverifiedResultBundle::new(&rejected));
        assert!(!rejected.exists());

        let accepted = temp.path().join("accepted.bundle");
        std::fs::write(&accepted, b"verified").expect("write accepted bundle");
        let mut guard = UnverifiedResultBundle::new(&accepted);
        guard.retain();
        drop(guard);
        assert_eq!(
            std::fs::read(&accepted).expect("read accepted"),
            b"verified"
        );
    }

    #[test]
    fn a_result_bundle_is_only_read_from_its_own_managed_directory() {
        // The path is chosen by the target, so a compromised or buggy one must
        // not be able to point this at an arbitrary file to exfiltrate.
        assert!(validate_managed_result_path(
            "/home/user",
            "job-1",
            "/home/user/.openbitfun/dispatch/workspaces/job-1/result.bundle"
        )
        .is_ok());
        for hostile in [
            "/home/user/.ssh/id_ed25519",
            "/home/user/.openbitfun/dispatch/workspaces/job-2/result.bundle",
            "/home/user/.openbitfun/dispatch/workspaces/job-1/../../../.ssh/id_ed25519",
            "/home/user/.openbitfun/dispatch/workspaces/job-1/current/secret",
        ] {
            assert!(
                validate_managed_result_path("/home/user", "job-1", hostile).is_err(),
                "must reject {hostile}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn dispatch_command_round_trips_quoted_paths_and_removes_request() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temp dir");
        let bin_dir = temp.path().join("bin with space");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");
        let cli = bin_dir.join("openbitfun's");
        std::fs::write(&cli, "#!/bin/sh\ncat\n").expect("fake CLI");
        let mut permissions = std::fs::metadata(&cli).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&cli, permissions).unwrap();

        let request = temp.path().join("request with ' quote.json");
        let payload = r#"{"prompt":"$(touch should-not-run); it's literal"}"#;
        std::fs::write(&request, payload).expect("request");
        let command =
            dispatch_command(&cli.to_string_lossy(), "submit", &request.to_string_lossy());
        assert!(
            !command.contains(payload),
            "request JSON must never be embedded in the shell command"
        );
        let output = std::process::Command::new("bash")
            .args(["-c", &command])
            .output()
            .expect("run quoted dispatch command");
        assert!(
            output.status.success(),
            "quoted command failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), payload);
        assert!(!request.exists(), "EXIT trap must remove the request");
        assert!(!temp.path().join("should-not-run").exists());
    }

    #[test]
    fn probe_prefers_the_managed_cli_over_an_incompatible_path_copy() {
        let script = probe_remote_target_script();
        let managed = script
            .find(r#"[ -x "$HOME/.local/bin/openbitfun" ]"#)
            .expect("managed CLI check");
        let path_lookup = script.find("command -v openbitfun").expect("PATH fallback");
        assert!(managed < path_lookup);
    }

    #[test]
    fn model_config_sync_merges_only_the_ai_keys() {
        let payload = serde_json::json!({
            "models": [{"id": "m1", "enabled": true}],
            "default_models": {"primary": "m1"},
            "agent_model_selections": {"mode":{"kind":"model","model_ref":"m1"},"subagents":{"default":{"kind":"inherit"},"builtin":{},"fork":{"kind":"inherit"}}},
            "model_pools": {"version":1,"pools":[]},
        });
        let payload = validate_model_config_payload(&payload).expect("valid payload");

        // Fresh target: a minimal document containing only the ai section.
        let fresh: Value =
            serde_json::from_str(&merge_model_config(None, payload).unwrap()).unwrap();
        assert_eq!(fresh["ai"]["models"][0]["id"], "m1");

        // Existing target: everything outside the synced keys is preserved.
        let existing = r#"{
            "editor": {"font_size": 11},
            "ai": {"models": [], "max_rounds": 7}
        }"#;
        let merged: Value =
            serde_json::from_str(&merge_model_config(Some(existing), payload).unwrap()).unwrap();
        assert_eq!(merged["editor"]["font_size"], 11);
        assert_eq!(merged["ai"]["max_rounds"], 7);
        assert_eq!(merged["ai"]["models"][0]["id"], "m1");
        assert_eq!(merged["ai"]["default_models"]["primary"], "m1");
        assert_eq!(
            merged["ai"]["agent_model_selections"],
            payload["agent_model_selections"]
        );
        assert_eq!(merged["ai"]["model_pools"], payload["model_pools"]);

        // A corrupt target config aborts instead of being replaced.
        assert!(merge_model_config(Some("not json"), payload).is_err());
        assert!(merge_model_config(Some("[]"), payload).is_err());
    }

    #[test]
    fn model_config_payload_rejects_unknown_keys_and_empty_catalogs() {
        assert!(validate_model_config_payload(&serde_json::json!({
            "models": [{"id": "m1"}],
            "tool_permissions": {}
        }))
        .is_err());
        assert!(validate_model_config_payload(&serde_json::json!({
            "models": [{"id": "m1"}],
            "task_models": {"session_title": {"kind": "inherit"}}
        }))
        .is_err());
        assert!(validate_model_config_payload(&serde_json::json!({ "models": [] })).is_err());
        assert!(validate_model_config_payload(&serde_json::json!("models")).is_err());
    }

    #[test]
    fn incompatible_dispatch_protocols_require_an_upgrade() {
        let capabilities = REQUIRED_DISPATCH_CAPABILITIES.clone();
        let compatible = serde_json::json!({
            "productId": openbitfun_services_core::product_identity::product_id(),
            "dataNamespace": openbitfun_services_core::product_identity::data_namespace(),
            "protocolVersion": DISPATCH_PROTOCOL_VERSION,
            "capabilities": capabilities,
        });
        assert!(dispatch_protocol_is_compatible(&compatible));

        let old = serde_json::json!({
            "productId": openbitfun_services_core::product_identity::product_id(),
            "dataNamespace": openbitfun_services_core::product_identity::data_namespace(),
            "protocolVersion": DISPATCH_PROTOCOL_VERSION - 1,
            "capabilities": capabilities,
        });
        assert!(!dispatch_protocol_is_compatible(&old));

        let missing = serde_json::json!({
            "productId": openbitfun_services_core::product_identity::product_id(),
            "dataNamespace": openbitfun_services_core::product_identity::data_namespace(),
            "protocolVersion": DISPATCH_PROTOCOL_VERSION,
            "capabilities": ["persistent_jobs", "cursor_events"],
        });
        assert!(!dispatch_protocol_is_compatible(&missing));

        let wrong_product = serde_json::json!({
            "productId": "acme",
            "dataNamespace": "acme",
            "protocolVersion": DISPATCH_PROTOCOL_VERSION,
            "capabilities": capabilities,
        });
        let error = validate_dispatch_protocol(&wrong_product, None)
            .expect_err("a different product must never share dispatch state");
        assert!(error
            .to_string()
            .contains("dispatch product is incompatible"));

        let mut reject_capabilities: Vec<&str> = REQUIRED_DISPATCH_CAPABILITIES
            .iter()
            .copied()
            .filter(|capability| !capability.starts_with("approval_"))
            .collect();
        reject_capabilities.push("approval_reject_and_report");
        let reject_only = serde_json::json!({
            "productId": openbitfun_services_core::product_identity::product_id(),
            "dataNamespace": openbitfun_services_core::product_identity::data_namespace(),
            "protocolVersion": DISPATCH_PROTOCOL_VERSION,
            "capabilities": reject_capabilities,
        });
        validate_dispatch_protocol(&reject_only, Some("reject-and-report"))
            .expect("selected policy is supported");
        assert!(validate_dispatch_protocol(&reject_only, Some("auto")).is_err());

        let mut unsafe_capabilities: Vec<&str> = REQUIRED_DISPATCH_CAPABILITIES
            .iter()
            .copied()
            .filter(|capability| {
                !capability.starts_with("approval_") && *capability != "dispatch_worker_cli_profile"
            })
            .collect();
        unsafe_capabilities.push("approval_reject_and_report");
        let unsafe_worker = serde_json::json!({
            "productId": openbitfun_services_core::product_identity::product_id(),
            "dataNamespace": openbitfun_services_core::product_identity::data_namespace(),
            "protocolVersion": DISPATCH_PROTOCOL_VERSION,
            "capabilities": unsafe_capabilities,
        });
        let error = validate_dispatch_protocol(&unsafe_worker, Some("reject-and-report"))
            .expect_err("a worker that can select product-full first must be rejected");
        assert!(
            error.to_string().contains("dispatch_worker_cli_profile"),
            "{error}"
        );
    }
}
