//! Durable attachment preparation shared by every Agent input surface.

use super::{decode_data_url, ImageContextData};
use crate::agentic::tools::framework::{build_tool_runtime_artifact_reference, ToolUseContext};
use crate::util::errors::{OpenBitFunError, OpenBitFunResult};
use base64::Engine;
use openbitfun_services_core::json_store::JsonFileStore;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn image_runtime_root(context: &ToolUseContext) -> OpenBitFunResult<PathBuf> {
    if context.workspace.is_some() {
        context.current_workspace_runtime_root()
    } else {
        Ok(crate::infrastructure::app_paths::try_get_path_manager_arc()?.user_data_dir())
    }
}

const IMAGE_DIRECTORY: &str = "attachments/images";

fn snapshot_filename(name: &str) -> bool {
    name.split_once('.').is_some_and(|(digest, extension)| {
        digest.len() == 64
            && digest.bytes().all(|b| b.is_ascii_hexdigit())
            && matches!(extension, "png" | "jpg" | "gif" | "webp" | "bmp")
    })
}

fn session_image_reference(relative: &str) -> OpenBitFunResult<String> {
    openbitfun_agent_tools::build_openbitfun_current_session_uri(relative)
        .map_err(|error| OpenBitFunError::validation(error.to_string()))
}

fn session_image_name(reference: &str) -> Option<String> {
    let parsed = openbitfun_agent_tools::parse_openbitfun_current_session_uri(reference).ok()?;
    let name = parsed
        .relative_path
        .strip_prefix(IMAGE_DIRECTORY)?
        .strip_prefix('/')?;
    snapshot_filename(name).then(|| name.to_owned())
}

pub(crate) async fn copy_session_image_reference(
    reference: &str,
    source_session_dir: &Path,
    target_session_dir: &Path,
) -> OpenBitFunResult<()> {
    let Some(name) = session_image_name(reference) else {
        return Ok(());
    };
    let source = source_session_dir.join(IMAGE_DIRECTORY).join(&name);
    let target = target_session_dir.join(IMAGE_DIRECTORY).join(name);
    if target.is_file() {
        return Ok(());
    }
    let bytes = match tokio::fs::read(&source).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(OpenBitFunError::io(error.to_string())),
    };
    persist_attachment(&target, bytes).await
}

fn image_reference(context: &ToolUseContext, name: &str) -> OpenBitFunResult<String> {
    build_tool_runtime_artifact_reference(
        &format!("{IMAGE_DIRECTORY}/{name}"),
        Some(&image_runtime_root(context)?),
        context.current_workspace_scope().as_deref(),
        context.should_emit_runtime_uri(),
    )
    .map_err(|error| OpenBitFunError::validation(error.to_string()))
}

pub(crate) fn is_saved_image(path: &str, context: &ToolUseContext) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or("");
    if session_image_name(path).is_some() {
        return context.session_id.is_some() && context.workspace.is_some();
    }
    snapshot_filename(name)
        && image_reference(context, name).is_ok_and(|reference| reference == path)
}

/// Save the exact frame, including observations that never had a source file.
pub(crate) async fn save_image(bytes: &[u8], context: &ToolUseContext) -> OpenBitFunResult<String> {
    let session = context
        .session_id
        .as_deref()
        .filter(|_| context.workspace.is_some());
    let root = if let Some(session_id) = session {
        openbitfun_core_types::validate_session_id(session_id)
            .map_err(OpenBitFunError::validation)?;
        context.current_workspace_session_dir(session_id)?
    } else {
        image_runtime_root(context)?
    };
    let (reference, _) = save_in_root(
        bytes,
        &root,
        context.current_workspace_scope().as_deref(),
        context.should_emit_runtime_uri(),
        session.is_some(),
    )
    .await?;
    Ok(reference)
}

pub(crate) async fn tool_image_references(
    images: &[crate::util::types::ToolImageAttachment],
    context: &ToolUseContext,
) -> OpenBitFunResult<Vec<String>> {
    let mut references = Vec::with_capacity(images.len());
    for image in images {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.data_base64)
            .map_err(|error| {
                OpenBitFunError::validation(format!("Invalid tool image data: {error}"))
            })?;
        references.push(save_image(&bytes, context).await?);
    }
    Ok(references)
}

pub(crate) async fn restore_tool_images(
    references: &[String],
    context: &ToolUseContext,
) -> OpenBitFunResult<Vec<crate::util::types::ToolImageAttachment>> {
    let mut images = Vec::with_capacity(references.len());
    for reference in references {
        if !is_saved_image(reference, context) {
            return Err(OpenBitFunError::validation(
                "Tool image snapshot belongs to another runtime or workspace",
            ));
        }
        let bytes = super::image_processing::read_image_in_context(reference, context).await?;
        let mime_type = super::detect_mime_type_from_bytes(&bytes, None)?;
        images.push(crate::util::types::ToolImageAttachment {
            mime_type,
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
    }
    Ok(images)
}

/// Freeze a path-only upload while its source still denotes the user's chosen image.
pub(crate) async fn prepare_input_attachments(
    images: &mut [ImageContextData],
    context: &ToolUseContext,
) -> OpenBitFunResult<()> {
    for image in images.iter_mut().filter(|image| image.data_url.is_none()) {
        let Some(path) = image.image_path.as_deref() else {
            continue;
        };
        if is_saved_image(path, context) {
            continue;
        }
        let bytes = super::image_processing::read_image_in_context(path, context).await?;
        let (reference, _) = save_in_root(
            &bytes,
            &image_runtime_root(context)?,
            context.current_workspace_scope().as_deref(),
            context.should_emit_runtime_uri(),
            false,
        )
        .await?;
        image.image_path = Some(reference);
    }
    prepare_inline_image_attachments(images, context).await
}

/// Inline pixels belong to the receiving Runtime, even in an SSH workspace.
/// Publish a stable artifact before admitting the message so text-only models
/// can call `analyze_image` and restored native multimodal turns can reload it.
/// Keep the inline payload for the transcript and for older message consumers.
pub(crate) async fn prepare_inline_image_attachments(
    images: &mut [ImageContextData],
    context: &ToolUseContext,
) -> OpenBitFunResult<()> {
    if !images.iter().any(|image| image.data_url.is_some()) {
        return Ok(());
    }
    let runtime_root = image_runtime_root(context)?;
    prepare_in_runtime_root(
        images,
        &runtime_root,
        context.current_workspace_scope().as_deref(),
        context.should_emit_runtime_uri(),
    )
    .await
}

async fn prepare_in_runtime_root(
    images: &mut [ImageContextData],
    runtime_root: &Path,
    workspace_scope: Option<&str>,
    emit_runtime_uri: bool,
) -> OpenBitFunResult<()> {
    for image in images {
        let Some(data_url) = image.data_url.as_deref() else {
            continue;
        };
        let (bytes, _) = decode_data_url(data_url)?;
        let (path, mime_type) = save_in_root(
            &bytes,
            runtime_root,
            workspace_scope,
            emit_runtime_uri,
            false,
        )
        .await?;
        image.image_path = Some(path);
        image.mime_type = mime_type.to_string();
    }
    Ok(())
}

async fn save_in_root(
    bytes: &[u8],
    runtime_root: &Path,
    workspace_scope: Option<&str>,
    emit_runtime_uri: bool,
    session_scoped: bool,
) -> OpenBitFunResult<(String, &'static str)> {
    let format = image::guess_format(bytes).map_err(|error| {
        OpenBitFunError::validation(format!("Invalid image attachment: {error}"))
    })?;
    let (extension, mime_type) = match format {
        image::ImageFormat::Png => ("png", "image/png"),
        image::ImageFormat::Jpeg => ("jpg", "image/jpeg"),
        image::ImageFormat::Gif => ("gif", "image/gif"),
        image::ImageFormat::WebP => ("webp", "image/webp"),
        image::ImageFormat::Bmp => ("bmp", "image/bmp"),
        _ => {
            return Err(OpenBitFunError::validation(format!(
                "Unsupported image attachment format: {format:?}. Use PNG, JPEG, GIF, WebP or BMP."
            )))
        }
    };
    let relative_path = format!("{IMAGE_DIRECTORY}/{:x}.{extension}", Sha256::digest(bytes));
    let path = runtime_root.join(&relative_path);
    // A replay or queued dispatch can prepare the same pixels twice. The
    // content address lets it reuse a complete, unchanged artifact.
    if tokio::fs::read(&path).await.ok().as_deref() != Some(bytes) {
        persist_attachment(&path, bytes.to_vec()).await?;
    }
    let reference = if session_scoped {
        session_image_reference(&relative_path)?
    } else {
        build_tool_runtime_artifact_reference(
            &relative_path,
            Some(runtime_root),
            workspace_scope,
            emit_runtime_uri,
        )
        .map_err(|error| OpenBitFunError::validation(error.to_string()))?
    };
    Ok((reference, mime_type))
}

#[cfg(test)]
pub(crate) fn test_image() -> ImageContextData {
    use base64::Engine;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(8, 6)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    ImageContextData {
        id: "image-1".into(),
        image_path: None,
        data_url: Some(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
        )),
        mime_type: "image/png".into(),
        metadata: Some(serde_json::json!({"name": "screenshot.png"})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn image_snapshot_freezes_path_uploads_within_the_submitting_workspace() {
        use crate::agentic::workspace::WorkspaceBinding;
        let root = tempfile::tempdir().unwrap();
        let mut image = test_image();
        let original = decode_data_url(&image.data_url.take().unwrap()).unwrap().0;
        let source = root.path().join("selected.png");
        tokio::fs::write(&source, &original).await.unwrap();
        image.image_path = Some(source.to_str().unwrap().into());
        let images = std::slice::from_mut(&mut image);
        let no_workspace = ToolUseContext::for_tool_listing(None, None);
        assert!(prepare_input_attachments(images, &no_workspace)
            .await
            .is_err());

        let workspace = WorkspaceBinding::new(Some("project".into()), root.path().into());
        let mut context = ToolUseContext::for_tool_listing(Some(workspace), None);
        context.custom_data.insert(
            "__openbitfun_test_runtime_root".into(),
            serde_json::json!(root.path().join("runtime")),
        );
        prepare_input_attachments(images, &context).await.unwrap();
        tokio::fs::remove_file(source).await.unwrap();
        let reference = image.image_path.as_deref().unwrap();
        assert!(is_saved_image(reference, &context));
        assert_eq!(
            super::super::image_processing::read_image_in_context(reference, &context)
                .await
                .unwrap(),
            original
        );
    }

    #[tokio::test]
    async fn inline_pixels_survive_redaction_and_reuse_the_same_artifact_on_retry() {
        let root = tempfile::tempdir().unwrap();
        let mut images = vec![test_image()];
        images[0].image_path = Some("/controller-only/wrong.png".into());
        images[0].id = "../../not-a-filename".into();
        let original = images[0].data_url.clone();
        prepare_in_runtime_root(&mut images, root.path(), None, false)
            .await
            .unwrap();
        let path = images[0].image_path.clone().unwrap();
        assert!(Path::new(&path).starts_with(root.path()));
        assert_eq!(images[0].data_url, original);
        assert_eq!(
            images[0].metadata.as_ref().unwrap()["name"],
            "screenshot.png"
        );
        prepare_in_runtime_root(&mut images, root.path(), None, false)
            .await
            .unwrap();
        assert_eq!(images[0].image_path.as_deref(), Some(path.as_str()));

        images[0].data_url = None;
        let restored: Vec<ImageContextData> =
            serde_json::from_value(serde_json::to_value(images).unwrap()).unwrap();
        let processed =
            super::super::process_image_contexts_for_provider(&restored, "openai", None)
                .await
                .unwrap();
        assert_eq!((processed[0].width, processed[0].height), (8, 6));
    }

    #[tokio::test]
    async fn remote_upload_references_runtime_artifacts_without_exposing_host_paths() {
        let root = tempfile::tempdir().unwrap();
        let mut images = vec![test_image(), test_image()];
        images[1].id = "image-2".into();
        prepare_in_runtime_root(&mut images, root.path(), Some("remote-workspace"), true)
            .await
            .unwrap();
        let reference = images[0].image_path.as_deref().unwrap();
        assert!(reference.starts_with("openbitfun://runtime/remote-workspace/attachments/images/"));
        assert!(!reference.contains(root.path().to_str().unwrap()));
        assert_eq!(images[0].image_path, images[1].image_path);
        assert_ne!(images[0].id, images[1].id);
        let parsed =
            crate::agentic::tools::workspace_paths::parse_openbitfun_runtime_uri(reference)
                .unwrap();
        assert!(root.path().join(parsed.relative_path).is_file());
    }

    #[tokio::test]
    async fn malformed_inline_payload_is_rejected_without_using_a_supplied_path() {
        let root = tempfile::tempdir().unwrap();
        for data_url in [
            "data:image/png,hello",
            "data:image/png;base64,???",
            "data:image/png;base64,aGVsbG8=",
        ] {
            let mut images = vec![test_image()];
            images[0].image_path = Some("/valid-looking/file.png".into());
            images[0].data_url = Some(data_url.into());
            assert!(
                prepare_in_runtime_root(&mut images, root.path(), None, false)
                    .await
                    .is_err()
            );
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

async fn persist_attachment(path: &Path, bytes: Vec<u8>) -> OpenBitFunResult<()> {
    JsonFileStore
        .write_bytes_atomic_strict(path, bytes)
        .await
        .map_err(|error| OpenBitFunError::io(format!("Failed to save image attachment: {error}")))
}
