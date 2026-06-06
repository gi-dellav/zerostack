pub mod slash;

use std::path::{Path, PathBuf};

use base64::Engine;
use compact_str::CompactString;
use rig::completion::ToolDefinition;
use rig::streaming::StreamingChat;
use rig::tool::Tool;

use crate::agent::tools::{ToolError, check_perm_path};
use crate::config::Config;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;
use crate::provider::{AnyClient, AnyModel};

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "heic", "heif"];

pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

fn guess_media_type(path: &Path) -> Option<rig::completion::message::ImageMediaType> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "png" => Some(rig::completion::message::ImageMediaType::PNG),
        "jpg" | "jpeg" => Some(rig::completion::message::ImageMediaType::JPEG),
        "gif" => Some(rig::completion::message::ImageMediaType::GIF),
        "webp" => Some(rig::completion::message::ImageMediaType::WEBP),
        "heic" => Some(rig::completion::message::ImageMediaType::HEIC),
        "heif" => Some(rig::completion::message::ImageMediaType::HEIF),
        "svg" => Some(rig::completion::message::ImageMediaType::SVG),
        _ => None,
    }
}

fn read_and_encode(path: &Path) -> Result<(Vec<u8>, String), String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    Ok((data, b64))
}

async fn describe_with_model(
    model: AnyModel,
    image: rig::completion::message::Image,
) -> Result<String, String> {
    match model {
        AnyModel::OpenRouter(m) => run_describe(m, image).await,
        AnyModel::OpenAI(m) => match m {
            crate::provider::OpenAiModel::Responses(m) => run_describe(m, image).await,
            crate::provider::OpenAiModel::Completions(m) => run_describe(m, image).await,
        },
        AnyModel::Anthropic(m) => run_describe(m, image).await,
        AnyModel::Gemini(m) => run_describe(m, image).await,
        AnyModel::Ollama(m) => run_describe(m, image).await,
    }
}

async fn run_describe<M>(model: M, image: rig::completion::message::Image) -> Result<String, String>
where
    M: rig::completion::CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
{
    let history = vec![rig::completion::Message::from(image)];

    let agent = rig::agent::AgentBuilder::new(model)
        .preamble("You are an image describer. Describe the image in detail, noting all relevant visual elements including text, UI elements, diagrams, code, errors, layout, colors, and structure. Be thorough and specific.")
        .build();

    let mut stream = agent
        .stream_chat("Describe this image in detail.", history)
        .multi_turn(1)
        .await;

    let mut response = String::new();
    use futures::StreamExt;
    while let Some(item) = stream.next().await {
        match item {
            Ok(rig::agent::MultiTurnStreamItem::StreamAssistantItem(
                rig::streaming::StreamedAssistantContent::Text(text),
            )) => response.push_str(&text.text),
            Ok(rig::agent::MultiTurnStreamItem::FinalResponse(res)) => {
                response = res.response().to_string();
                break;
            }
            Err(e) => return Err(format!("vision model error: {e}")),
            _ => {}
        }
    }

    if response.is_empty() {
        return Err("vision model returned empty description".to_string());
    }

    Ok(response)
}

async fn describe_single_image(
    client: &AnyClient,
    model_name: &str,
    image_path: &Path,
) -> Result<String, String> {
    let (_, b64) = read_and_encode(image_path)?;
    let media_type = guess_media_type(image_path);

    let image = rig::completion::message::Image {
        data: rig::completion::message::DocumentSourceKind::base64(&b64),
        media_type,
        detail: None,
        additional_params: None,
    };

    let model = client.completion_model(model_name.to_string());
    describe_with_model(model, image).await
}

pub async fn describe_images(
    image_paths: &[PathBuf],
    vision_client: &AnyClient,
    vision_model_name: &str,
) -> Result<String, String> {
    let mut descriptions = String::new();

    for (i, path) in image_paths.iter().enumerate() {
        let desc = describe_single_image(vision_client, vision_model_name, path).await?;
        if !descriptions.is_empty() {
            descriptions.push('\n');
        }
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_else(|| path.to_string_lossy().into());
        descriptions.push_str(&format!("[Image {}: {}]\n{}", i + 1, filename, desc));
    }

    Ok(descriptions)
}

pub fn create_vision_client(cfg: &Config) -> Result<AnyClient, String> {
    let vision_model_name = cfg.vision_model.as_deref().unwrap_or("qwen3.5-27b-vision");

    let quick_models = crate::config::quick_models_map(cfg);
    let qm = quick_models
        .get(vision_model_name)
        .ok_or_else(|| format!("vision_model '{vision_model_name}' not found in quick_models"))?;

    crate::provider::create_client(
        &qm.provider,
        None,
        &cfg.custom_providers_map(),
        cfg.api_keys.as_ref(),
    )
    .map_err(|e| format!("failed to create vision client: {e}"))
}

pub struct ViewImageTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
    pub vision_client: AnyClient,
    pub vision_model_name: CompactString,
}

impl ViewImageTool {
    pub fn new(
        permission: Option<PermCheck>,
        ask_tx: Option<AskSender>,
        vision_client: AnyClient,
        vision_model_name: &str,
    ) -> Self {
        ViewImageTool {
            permission,
            ask_tx,
            vision_client,
            vision_model_name: CompactString::new(vision_model_name),
        }
    }
}

#[derive(serde::Deserialize)]
pub struct ViewImageArgs {
    pub path: String,
}

impl Tool for ViewImageTool {
    const NAME: &'static str = "view_image";

    type Error = ToolError;
    type Args = ViewImageArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "view_image".to_string(),
            description: "View and describe an image file. Use this to examine screenshots, UI mockups, diagrams, photos, or any other image. Returns a detailed text description of the image contents.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the image file to view and describe"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: ViewImageArgs) -> Result<String, ToolError> {
        let path_str = crate::fs::expand_tilde(&args.path);
        let path = Path::new(&path_str);

        let coaching =
            check_perm_path(&self.permission, &self.ask_tx, "view_image", &path_str).await?;

        let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        if !resolved.is_file() {
            return Err(ToolError::Msg(format!(
                "not a file: {}",
                resolved.display()
            )));
        }

        let description =
            describe_single_image(&self.vision_client, &self.vision_model_name, &resolved)
                .await
                .map_err(ToolError::Msg)?;

        let mut output = if let Some(c) = coaching {
            c + "\n"
        } else {
            String::new()
        };
        output.push_str(&format!("[Image: {}]\n", resolved.display()));
        output.push_str(&description);
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_image_path_png() {
        assert!(is_image_path(Path::new("screenshot.png")));
    }

    #[test]
    fn test_is_image_path_jpg() {
        assert!(is_image_path(Path::new("photo.jpg")));
        assert!(is_image_path(Path::new("photo.jpeg")));
    }

    #[test]
    fn test_is_image_path_case_insensitive() {
        assert!(is_image_path(Path::new("IMAGE.PNG")));
        assert!(is_image_path(Path::new("Photo.JPG")));
    }

    #[test]
    fn test_is_image_path_rejects_non_images() {
        assert!(!is_image_path(Path::new("file.txt")));
        assert!(!is_image_path(Path::new("code.rs")));
        assert!(!is_image_path(Path::new("readme.md")));
    }

    #[test]
    fn test_is_image_path_rejects_no_extension() {
        assert!(!is_image_path(Path::new("Makefile")));
    }

    #[test]
    fn test_guess_media_type() {
        assert_eq!(
            guess_media_type(Path::new("img.png")),
            Some(rig::completion::message::ImageMediaType::PNG)
        );
        assert_eq!(
            guess_media_type(Path::new("img.jpg")),
            Some(rig::completion::message::ImageMediaType::JPEG)
        );
        assert_eq!(
            guess_media_type(Path::new("img.gif")),
            Some(rig::completion::message::ImageMediaType::GIF)
        );
        assert_eq!(guess_media_type(Path::new("file.txt")), None);
    }
}
