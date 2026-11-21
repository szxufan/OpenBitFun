//! Seal inline figures at turn completion and replay legacy edits from recorded tool data.
//! Never consult mutable files when replaying history.
use super::{PersistenceManager, SessionStorageLayout, StoredDialogTurnFile};
use crate::agentic::inline_figure::{figure_uri, MAX_SOURCE_BYTES};
use crate::util::errors::OpenBitFunResult;
use openbitfun_agent_tools::effective_tool_invocation;
use openbitfun_services_core::session::{DialogTurnData, StoredSessionMetadataFile, ToolItemData};
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path};

#[derive(Clone, Hash, PartialEq, Eq)]
struct Figure {
    uri: String,
    subagent: Option<String>,
}

enum Revision {
    Snapshot(Option<String>),
    Edit {
        old: String,
        new: String,
        count: usize,
    },
}

impl Revision {
    fn recorded(item: &ToolItemData, local_root: Option<&Path>) -> Option<(Figure, Self)> {
        let receipt = item.tool_result.as_ref().filter(|r| r.success)?;
        if item.status.as_deref().is_some_and(|s| s != "completed") {
            return None;
        }
        let (name, args) = effective_tool_invocation(&item.tool_name, &item.tool_call.input);
        if !matches!(name, "Write" | "Edit") {
            return None;
        }
        let result = &receipt.result;
        let path = result.get("file_path")?.as_str()?;
        let figure = Figure {
            uri: figure_uri(path, local_root)?,
            subagent: item.subagent_session_id.clone(),
        };
        if result["visualization_checked"] == false {
            return Some((figure, Self::Snapshot(None)));
        }
        if result["visualization_checked"] == true {
            if let Some(compiled) = result.get("visualization_rendered_source") {
                let source = compiled.as_str().filter(|s| {
                    !s.is_empty()
                        && s.len() <= MAX_SOURCE_BYTES
                        && Some(s.len() as u64) == result["visualization_rendered_bytes"].as_u64()
                });
                return Some((figure, Self::Snapshot(source.map(str::to_owned))));
            }
        }
        let size = result
            .get("content_bytes")
            .or_else(|| result.get("bytes_written"))
            .and_then(Value::as_u64);
        let snapshot = |source: Option<&str>| {
            Self::Snapshot(
                source
                    .filter(|s| {
                        !s.is_empty() && s.len() <= MAX_SOURCE_BYTES && Some(s.len() as u64) == size
                    })
                    .map(str::to_owned),
            )
        };
        let revision = if name == "Write" {
            let source = args["payload"]
                .as_str()
                .and_then(|p| p.split_once('\n'))
                .filter(|(line, _)| line.trim_end_matches('\r').strip_prefix("+++ ") == Some(path))
                .map(|(_, source)| source);
            snapshot(source)
        } else if args["file_path"].as_str() != Some(path) {
            Self::Snapshot(None)
        } else if let Some(source) = result.get("visualization_source") {
            snapshot(source.as_str())
        } else if let (Some(old), Some(new), Some(count)) = (
            args["old_string"].as_str(),
            args["new_string"].as_str(),
            result["match_count"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok()),
        ) {
            if old.is_empty()
                || old.len() > MAX_SOURCE_BYTES
                || new.len() > MAX_SOURCE_BYTES
                || result["old_string"] != old
                || result["new_string"] != new
                || count == 0
                || (count != 1 && args["replace_all"] != true)
            {
                Self::Snapshot(None)
            } else {
                Self::Edit {
                    old: old.into(),
                    new: new.into(),
                    count,
                }
            }
        } else {
            Self::Snapshot(None)
        };
        Some((figure, revision))
    }

    fn apply(self, source: Option<String>) -> Option<String> {
        match self {
            Self::Snapshot(source) => source,
            Self::Edit { old, new, count } => {
                let source = source?;
                if source.matches(&old).count() != count {
                    return None;
                }
                let size = source
                    .len()
                    .checked_sub(old.len().checked_mul(count)?)?
                    .checked_add(new.len().checked_mul(count)?)?;
                (size <= MAX_SOURCE_BYTES).then(|| source.replace(&old, &new))
            }
        }
    }
}

/// Read a regular file under the exact, producing session directory.
/// The runtime owns the artifact directory, even when the workspace itself is remote.
fn completed_figure_source(
    session: &Path,
    filename: &str,
    modified_since: Option<u64>,
) -> Option<String> {
    use openbitfun_services_core::bounded_fs::{read_bounded_text, BoundedTextRead};
    let root = session.parent()?.canonicalize().ok()?;
    let path = root
        .join(session.file_name()?)
        .join("artifacts/visualizations/v2")
        .join(filename);
    if path.canonicalize().ok()? != path {
        return None;
    }
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES as u64 {
        return None;
    }
    if let Some(start) = modified_since {
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        if modified.as_millis() < start as u128 {
            return None;
        }
    }
    match read_bounded_text(&path, MAX_SOURCE_BYTES).ok()? {
        BoundedTextRead::Content(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

/// Older writers omitted the URI on empty or oversized writes. Recover that
/// rejection from recorded sizes without consulting the current file.
fn restore_rejected_revisions(turn: &mut DialogTurnData, local_root: Option<&Path>) {
    let mut rejected = std::collections::HashSet::new();
    for item in turn.model_rounds.iter_mut().flat_map(|r| &mut r.tool_items) {
        if item.subagent_session_id.is_some()
            || item.is_subagent_item == Some(true)
            || item
                .status
                .as_deref()
                .is_some_and(|status| status != "completed")
        {
            continue;
        }
        let (name, _) = effective_tool_invocation(&item.tool_name, &item.tool_call.input);
        if !matches!(name, "Write" | "Edit") {
            continue;
        }
        let Some(receipt) = item.tool_result.as_mut().filter(|result| result.success) else {
            continue;
        };
        let result = &mut receipt.result;
        let Some(uri) = result["visualization_uri"]
            .as_str()
            .and_then(|s| figure_uri(s, None))
            .or_else(|| {
                result["file_path"]
                    .as_str()
                    .and_then(|s| figure_uri(s, local_root))
            })
        else {
            continue;
        };
        if result["visualization_uri"].is_null()
            && result
                .get("content_bytes")
                .and_then(Value::as_u64)
                .is_some_and(|n| n == 0 || n > MAX_SOURCE_BYTES as u64)
        {
            result["visualization_uri"] = json!(uri);
            result["visualization_checked"] = json!(false);
        }
        if result["visualization_checked"] == false {
            rejected.insert(uri);
        } else {
            rejected.remove(&uri);
        }
    }
    for uri in rejected {
        if let Some(snapshot) = turn.published_figures.get_mut(&uri) {
            *snapshot = None;
        }
    }
}

impl PersistenceManager {
    /// Terminal history reconciliation sends this snapshot to live readers as well as replay.
    /// Keep the original tool input/result intact for readers displaying earlier messages.
    pub(crate) async fn seal_completed_figures(
        &self,
        workspace_path: &Path,
        turn: &mut DialogTurnData,
    ) {
        use openbitfun_services_core::session::PublishedFigure;
        use std::collections::BTreeSet;
        restore_rejected_revisions(turn, None);
        if !turn.published_figures.is_empty() {
            return;
        }
        let root = self
            .session_layout(workspace_path)
            .session_dir(&turn.session_id);
        let mut registered = BTreeSet::new();
        let mut checked = HashMap::new();
        let mut children = BTreeSet::new();
        let mut used_skill = false;
        for item in turn.model_rounds.iter().flat_map(|r| &r.tool_items) {
            let Some(receipt) = item.tool_result.as_ref().filter(|r| r.success) else {
                continue;
            };
            let (name, _) = effective_tool_invocation(&item.tool_name, &item.tool_call.input);
            let child = item.subagent_session_id.is_some() || item.is_subagent_item == Some(true);
            if name == "Skill" && !child && receipt.result["skill_name"] == "visualize" {
                used_skill = true;
            }
            if !matches!(name, "Write" | "Edit") {
                continue;
            }
            if let Some(uri) = receipt.result["visualization_uri"]
                .as_str()
                .and_then(|s| figure_uri(s, None))
            {
                if child {
                    children.insert(uri);
                } else {
                    checked.remove(&uri);
                    if let Some(passed) =
                        receipt.result["visualization_checked"]
                            .as_bool()
                            .or_else(|| {
                                (receipt.result["visualization_check_deferred"] == true)
                                    .then_some(true)
                            })
                    {
                        let exact = Revision::recorded(item, Some(&root))
                            .and_then(|(_, revision)| revision.apply(None));
                        let bindings = receipt.result["visualization_bindings"]
                            .as_object()
                            .cloned();
                        let deferred = receipt.result["visualization_check_deferred"] == true;
                        checked.insert(uri.clone(), (exact.filter(|_| passed), bindings, deferred));
                    }
                    registered.insert(uri);
                }
            }
        }
        if registered.is_empty() && !used_skill {
            return;
        }
        let reply = turn
            .model_rounds
            .iter()
            .rev()
            .flat_map(|r| r.text_items.iter().rev())
            .find(|item| item.subagent_session_id.is_none() && item.is_subagent_item != Some(true));
        let Some(reply) = reply else {
            return;
        };
        let mut references: BTreeSet<_> = reply
            .content
            .lines()
            .filter_map(|line| {
                let body = line.trim().strip_prefix("visualize")?.strip_suffix("")?;
                let reference: Value = serde_json::from_str(body).ok()?;
                figure_uri(reference["path"].as_str()?, None)
            })
            .collect();
        if references.is_empty() {
            references.clone_from(&registered);
        }
        for uri in references.difference(&children) {
            let attested = registered.contains(uri);
            if !attested && !used_skill {
                continue;
            }
            // For a shell-created figure, only an explicit reference to a file
            // actually modified in this turn may publish it.
            let since = (!attested).then_some(turn.start_time);
            let filename = uri.rsplit('/').next().unwrap().to_owned();
            let session = root.clone();
            let (source, bindings, inspection_deferred) = if let Some(exact) = checked.remove(uri) {
                exact
            } else {
                let source = tokio::task::spawn_blocking(move || {
                    completed_figure_source(&session, &filename, since)
                })
                .await
                .ok()
                .flatten();
                (source, None, false)
            };
            turn.published_figures.insert(
                uri.clone(),
                source.map(|source| PublishedFigure {
                    inspection_deferred,
                    bindings,
                    content_bytes: source.len(),
                    source,
                }),
            );
        }
    }

    async fn prior_figure_source(
        &self,
        root: &Path,
        session: &str,
        before: usize,
        figure: &Figure,
        local_root: Option<&Path>,
    ) -> Option<String> {
        let layout = SessionStorageLayout::new(root.parent()?);
        let mut edits = Vec::new();
        let paths = layout.list_indexed_turn_paths(session).await.ok()?;
        for (index, path) in paths.into_iter().rev().filter(|(i, _)| *i < before) {
            let file = self
                .read_json_optional::<StoredDialogTurnFile>(&path)
                .await
                .ok()??;
            if file.turn.session_id != session || file.turn.turn_index != index {
                return None;
            }
            for item in file
                .turn
                .model_rounds
                .iter()
                .rev()
                .flat_map(|round| round.tool_items.iter().rev())
            {
                let Some((key, revision)) = Revision::recorded(item, local_root) else {
                    continue;
                };
                if key != *figure {
                    continue;
                }
                match revision {
                    Revision::Snapshot(source) => {
                        return edits
                            .into_iter()
                            .rev()
                            .fold(source, |s, edit: Revision| edit.apply(s))
                    }
                    edit => edits.push(edit),
                }
            }
        }
        None
    }

    pub(super) async fn read_turn_for_display(
        &self,
        path: &Path,
    ) -> OpenBitFunResult<Option<StoredDialogTurnFile>> {
        let mut file = self
            .read_json_optional::<StoredDialogTurnFile>(path)
            .await?;
        if let (Some(file), Some(root)) = (&mut file, path.parent().and_then(Path::parent)) {
            if root.file_name().and_then(|s| s.to_str()) == Some(file.turn.session_id.as_str()) {
                self.restore_figure_receipts(root, &mut file.turn).await;
            }
        }
        Ok(file)
    }

    async fn restore_figure_receipts(&self, root: &Path, turn: &mut DialogTurnData) {
        // Current hosts already store self-contained receipts. Only old edits need earlier turns.
        let needs_restore = turn
            .model_rounds
            .iter()
            .flat_map(|r| &r.tool_items)
            .any(|item| {
                let Some(result) = item
                    .tool_result
                    .as_ref()
                    .filter(|r| r.success)
                    .map(|r| &r.result)
                else {
                    return false;
                };
                let Some(path) = result["file_path"].as_str() else {
                    return false;
                };
                let (name, _) = effective_tool_invocation(&item.tool_name, &item.tool_call.input);
                matches!(name, "Write" | "Edit")
                    && result.get("visualization_uri").is_none()
                    && path.ends_with(".html")
                    && (Path::new(path).is_absolute()
                        || (name == "Edit" && result.get("visualization_source").is_none()))
                    && figure_uri(path, Some(root)).is_some()
            });
        if !needs_restore {
            restore_rejected_revisions(turn, None);
            return;
        }
        // A workspace file on an SSH machine can have the same spelling as a host path.
        // Only the producing local workspace can attest legacy absolute destinations.
        let Some(sessions_root) = root.parent() else {
            return;
        };
        let metadata_path =
            SessionStorageLayout::new(sessions_root).metadata_path(&turn.session_id);
        let local_root = self
            .read_json_optional::<StoredSessionMetadataFile>(&metadata_path)
            .await
            .ok()
            .flatten()
            .filter(|file| {
                file.metadata.session_id == turn.session_id
                    && file.metadata.workspace_hostname.as_deref()
                        == Some(super::LOCAL_WORKSPACE_SSH_HOST)
            })
            .map(|_| root);
        restore_rejected_revisions(turn, local_root);
        let mut sources = HashMap::new();
        for item in turn
            .model_rounds
            .iter_mut()
            .flat_map(|round| &mut round.tool_items)
        {
            let Some((key, revision)) = Revision::recorded(item, local_root) else {
                continue;
            };
            let receipt = &mut item.tool_result.as_mut().unwrap().result;
            let legacy_edit = matches!(revision, Revision::Edit { .. });
            let base = if legacy_edit && !sources.contains_key(&key) {
                self.prior_figure_source(root, &turn.session_id, turn.turn_index, &key, local_root)
                    .await
            } else {
                sources.remove(&key).flatten()
            };
            let source = revision.apply(base);
            if let Some(html) = &source {
                receipt["visualization_uri"] = json!(key.uri);
                if legacy_edit {
                    receipt["content_bytes"] = json!(html.len());
                    receipt["visualization_source"] = json!(html);
                }
            }
            sources.insert(key, source);
        }
    }
}
