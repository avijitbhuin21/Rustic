//! Media-generation tools: `image_create`, `video_create`, `animate`.
//!
//! These are client-side tools — every supported provider is called over
//! HTTP from the executor host (not server-side via the model API). Each
//! tool is conditionally registered only when the user has filled in the
//! matching `MediaModelEntry` under `ToolConfig.media`.
//!
//! Outputs (PNG / JPEG / MP4) are written under
//! `<project_root>/.rustic/generated/` with a timestamped, slugified file
//! name. The tool result returned to the model is a JSON envelope that
//! lists the saved file paths plus the prompt — the frontend's chat-view
//! parses this for `image_create` / `video_create` / `animate` and renders
//! the media inline above the standard tool card.

use crate::config::{AiConfig, ProviderType, ToolConfig};
use crate::tools::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use anyhow::Result;
use base64::Engine;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Top-level folder for image outputs. Images are namespaced under
/// `<root>/.rustic/generated_images/<task_id>/<timestamp>-<slug>.png`. The
/// `task_id` segment keeps every chat's media together so the user can find
/// or wipe them per-conversation, and so the agent can reference them by a
/// stable, predictable path across turns.
const GENERATED_IMAGES_DIR: &str = ".rustic/generated_images";
/// Same layout for video / animation outputs.
const GENERATED_VIDEOS_DIR: &str = ".rustic/generated_videos";

/// Estimated USD cost of generating one image with `model`. These match the
/// public per-image rates each provider advertised at the cutoff and are
/// intentionally conservative — exact spend depends on quality / size knobs
/// the model picks, but the order of magnitude is right. Update when the
/// providers change their price list. Unknown models fall back to $0.04 —
/// the median of the bunch.
fn image_unit_cost_usd(model: &str) -> f64 {
    let m = model.to_lowercase();
    // OpenAI image families
    if m.starts_with("gpt-image-1-mini") { return 0.011; }
    if m.starts_with("gpt-image-1.5") || m.starts_with("gpt-image-2") { return 0.05; }
    if m.starts_with("gpt-image-1") { return 0.042; }     // medium-quality default
    if m.starts_with("dall-e-3") { return 0.040; }
    if m.starts_with("dall-e-2") { return 0.020; }
    // Gemini / Imagen
    if m.contains("gemini-2.5-flash-image") || m.contains("nano-banana") { return 0.039; }
    if m.starts_with("imagen-4") { return 0.040; }
    if m.starts_with("imagen-3") { return 0.020; }
    // OpenRouter routed equivalents — match the upstream model's price
    if m.contains("gemini-2.5-flash-image") || m.contains("gemini-3-flash-image") { return 0.039; }
    if m.contains("seedream") { return 0.030; }
    if m.contains("gpt-5.4-image-2") { return 0.050; }
    // Unknown model — median fallback so the running total isn't zero.
    0.04
}

/// Estimated USD cost per second of generated video for `model`. Sora 2 and
/// Veo families are the well-priced public benchmarks; OpenRouter routes
/// follow the upstream rate. Per-second figures here are list price ranges
/// from each provider's docs at the time of writing — same caveat as
/// `image_unit_cost_usd`: this is a budget estimate, not an exact bill.
fn video_unit_cost_per_second_usd(model: &str) -> f64 {
    let m = model.to_lowercase();
    // OpenAI Sora
    if m.starts_with("sora-2-pro") { return 0.50; }
    if m.starts_with("sora-2") { return 0.10; }
    if m.starts_with("sora-1") { return 0.30; }
    // Google Veo
    if m.contains("veo-3.1") { return 0.40; }
    if m.contains("veo-3.0-fast") || m.contains("veo-3-fast") { return 0.15; }
    if m.contains("veo-3.0") || m.contains("veo-3") { return 0.40; }
    if m.contains("veo-2") { return 0.35; }
    // OpenRouter routes — same model strings as above usually flow through.
    if m.contains("seedance-2") { return 0.20; }
    if m.contains("seedance-1") { return 0.15; }
    if m.contains("wan-2") { return 0.10; }
    // Unknown — assume a mid-tier rate so total reads as nonzero.
    0.20
}

/// Default clip length used for cost estimation when the model doesn't
/// surface the exact length and the user didn't pass `duration_seconds`.
/// Most providers default to ~4-8s; pick 6 as a sensible mid-point.
const DEFAULT_VIDEO_SECONDS_FOR_COST: u32 = 6;

fn estimate_image_cost(model: &str, count: u32) -> f64 {
    image_unit_cost_usd(model) * count as f64
}

fn estimate_video_cost(model: &str, count: u32, duration: Option<u32>) -> f64 {
    let secs = duration.unwrap_or(DEFAULT_VIDEO_SECONDS_FOR_COST) as f64;
    video_unit_cost_per_second_usd(model) * count as f64 * secs
}

/// Deposit `cost` into the per-turn tool-cost accumulator. The executor
/// drains this after every tool batch and folds it into `TaskCost`.
fn deposit_cost(context: &ToolContext, cost: f64) {
    if cost <= 0.0 { return; }
    if let Ok(mut sink) = context.tool_cost_sink.lock() {
        *sink += cost;
    }
}

/// Builtin ToolDefs for media tools. Returned by the executor only when the
/// corresponding `MediaModelEntry` has both a provider_key and a model id.
pub fn definitions_for(config: &ToolConfig) -> Vec<ToolDef> {
    let mut defs = Vec::new();

    if config.media.image.is_configured() {
        let max = config.media.image.effective_max();
        defs.push(ToolDef {
            name: "image_create".to_string(),
            description: format!(
                "Generate or edit images from a natural-language prompt. \
                Without `image_paths` this is text-to-image. With one or more \
                `image_paths` it becomes image-to-image / inline editing: the \
                provider takes the source image(s) plus the prompt and returns \
                an edited variant (e.g. \"change the sky to sunset\", \"swap \
                the car for a bike\", \"combine these two shots into one scene\"). \
                You can also chain — pass a path from an earlier `image_create` \
                output to iterate on it. Saves the result(s) under \
                `.rustic/generated_images/` inside the project and returns the \
                saved paths. The user has configured a maximum of {} image(s) \
                per call — do not exceed this.",
                max
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Describe the image (text-to-image) OR the edit you want applied (image-to-image). For edits, be specific about what to change and what to keep, e.g. \"replace the background with a forest, keep the subject identical\"."
                    },
                    "count": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": max,
                        "description": format!("How many images to generate (1..={}). Defaults to 1.", max)
                    },
                    "size": {
                        "type": "string",
                        "description": "Optional size hint. Use one of \"1024x1024\", \"1024x1536\" (portrait), or \"1536x1024\" (landscape). Provider may ignore or remap unsupported sizes."
                    },
                    "image_paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional. Paths to one or more source images (PNG / JPEG / WEBP). Each path may be either project-relative (`assets/photo.png`, `.rustic/uploaded/<task>/pasted.png`) OR absolute (`C:\\Users\\me\\Pictures\\dog.jpg`, `/home/me/pic.png`) — both are accepted, so you can edit an image the user pasted into chat (saved under `.rustic/uploaded/`), an image already in the project, or one anywhere else on the local disk. When present, the tool runs in image-to-image / editing mode: the model edits or composes from these images using `prompt` as the instruction. Most providers accept a single image; OpenAI gpt-image-* and Gemini also accept multiple. You may also pass a path from a previous `image_create` result to iterate further on it."
                    }
                },
                "required": ["prompt"]
            }),
        });
    }

    if config.media.video.is_configured() {
        let max = config.media.video.effective_max();
        defs.push(ToolDef {
            name: "video_create".to_string(),
            description: format!(
                "Generate a short video clip from a natural-language prompt. \
                Saves the result(s) under `.rustic/generated_videos/<task_id>/` and \
                returns the saved paths. The user has configured a maximum of {} \
                clip(s) per call. Generation can take 1–5 minutes; the tool blocks \
                until the file is saved or generation fails. You may pass an \
                optional `image_path` (project-relative) — including any image \
                you generated with `image_create` — to use as the first frame.",
                max
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Describe the video — subject, action, camera movement, style."
                    },
                    "count": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": max,
                        "description": format!("Number of clips (1..={}). Defaults to 1.", max)
                    },
                    "duration_seconds": {
                        "type": "integer",
                        "minimum": 2,
                        "maximum": 12,
                        "description": "Optional clip length in seconds. Providers may clamp to their supported range."
                    },
                    "aspect_ratio": {
                        "type": "string",
                        "description": "Optional aspect ratio, e.g. \"16:9\" or \"9:16\"."
                    },
                    "image_path": {
                        "type": "string",
                        "description": "Optional path to an image to use as the first frame. May be project-relative (e.g. `.rustic/uploaded/<task>/pasted.png` for an image the user pasted into chat, or a path returned by `image_create`) or absolute (e.g. `C:\\Users\\me\\Pictures\\dog.jpg`). Use this to turn an existing or freshly-edited image into a video without calling `animate` separately."
                    }
                },
                "required": ["prompt"]
            }),
        });
    }

    let animate_entry = config.media.effective_animate();
    if animate_entry.is_configured() {
        let max = animate_entry.effective_max();
        defs.push(ToolDef {
            name: "animate".to_string(),
            description: format!(
                "Animate an existing image into a short video clip. Takes a path \
                to an image inside the project and a prompt describing the \
                motion. Saves the result under `.rustic/generated/` and returns \
                the saved path. Max {} clip(s) per call.",
                max
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "image_path": {
                        "type": "string",
                        "description": "Path to an image (PNG / JPEG / WEBP) to use as the first frame. May be project-relative (e.g. `.rustic/uploaded/<task>/pasted.png` for a pasted attachment, or any image already in the project) or absolute (e.g. `C:\\Users\\me\\Pictures\\dog.jpg`)."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Describe how the image should move — camera pan, subject action, mood."
                    },
                    "count": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": max,
                        "description": format!("Number of clips (1..={}). Defaults to 1.", max)
                    },
                    "duration_seconds": {
                        "type": "integer",
                        "minimum": 2,
                        "maximum": 12,
                        "description": "Optional clip length in seconds."
                    }
                },
                "required": ["image_path", "prompt"]
            }),
        });
    }

    defs
}

/// Tool dispatch entrypoint. Routed from `BuiltinTools::execute` for
/// `image_create` / `video_create` / `animate`.
pub async fn execute(
    name: &str,
    tool_use_id: &str,
    params: Value,
    context: &ToolContext,
) -> Result<ToolOutput> {
    match name {
        "image_create" => run_image_create(params, tool_use_id, context).await,
        "video_create" => run_video_create(params, tool_use_id, context).await,
        "animate" => run_animate(params, tool_use_id, context).await,
        _ => Ok(error(format!("Unknown media tool: {}", name))),
    }
}

// ── tool runners ────────────────────────────────────────────────────────────

async fn run_image_create(
    params: Value,
    tool_use_id: &str,
    context: &ToolContext,
) -> Result<ToolOutput> {
    let entry = &context.tool_config.media.image;
    if !entry.is_configured() {
        return Ok(error(
            "image_create is not configured. Open Settings → Tools → Media to pick a provider and model.".to_string(),
        ));
    }
    let prompt = match params.get("prompt").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return Ok(error("image_create requires a non-empty `prompt`.".to_string())),
    };
    let count = clamp_count(params.get("count").and_then(|v| v.as_u64()), entry.effective_max());
    let size = params
        .get("size")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // Optional image inputs. Three model-side quirks to tolerate:
    //
    //   1. Singular string instead of array:                 "image_paths": "a.png"
    //   2. Native array of stringified arrays (observed!):   "image_paths": ["[\"a.png\"]"]
    //   3. Whole field stringified:                          "image_paths": "[\"a.png\"]"
    //
    // OpenAI-compat proxies fronting non-OpenAI models tend to double-encode
    // array fields. Without this normalization the bracketed JSON blob ends
    // up treated as a literal filename and the file lookup fails with
    // `["..."] not found`. We flatten each entry recursively so any depth
    // of stringification unwraps cleanly.
    fn flatten_path_entry(raw: &str, out: &mut Vec<String>) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(trimmed) {
                for v in arr {
                    if let Some(s) = v.as_str() {
                        flatten_path_entry(s, out);
                    }
                }
                return;
            }
        }
        out.push(trimmed.to_string());
    }
    let mut image_paths_raw: Vec<String> = Vec::new();
    match params.get("image_paths") {
        Some(Value::Array(arr)) => {
            for v in arr {
                if let Some(s) = v.as_str() {
                    flatten_path_entry(s, &mut image_paths_raw);
                }
            }
        }
        Some(Value::String(s)) => flatten_path_entry(s, &mut image_paths_raw),
        _ => {}
    }
    let mut input_images_owned: Vec<(String, &'static str)> = Vec::new();
    for rel in &image_paths_raw {
        // `PathBuf::join` returns the right-hand side verbatim when it is
        // absolute, so this transparently handles both project-relative and
        // absolute inputs (e.g. `C:\Users\me\Pictures\dog.jpg`).
        let abs_path = context.project_root.join(rel);
        if !abs_path.exists() || !abs_path.is_file() {
            return Ok(error(format!(
                "image_create: source image `{}` not found (looked for {}). Provide either a project-relative path or an absolute path to an existing file.",
                rel,
                abs_path.display()
            )));
        }
        let bytes = tokio::fs::read(&abs_path).await.map_err(|e| anyhow::anyhow!(e))?;
        let mime = guess_image_mime(&abs_path);
        input_images_owned.push((
            base64::engine::general_purpose::STANDARD.encode(&bytes),
            mime,
        ));
    }
    let input_images: Vec<(&str, &str)> = input_images_owned
        .iter()
        .map(|(b, m)| (b.as_str(), *m))
        .collect();

    let provider = match find_provider(&context.ai_config, &entry.provider_key) {
        Some(p) => p,
        None => return Ok(error(format!(
            "image_create: provider `{}` is not configured. Add the API key under Settings → AI Providers.",
            entry.provider_key
        ))),
    };

    let mode_label = if input_images.is_empty() { "Generating" } else { "Editing" };
    context.emit_progress(tool_use_id, &format!("{} {} image(s)…", mode_label, count));

    let outputs = match provider.kind {
        ProviderType::OpenAi => {
            openai_generate_images(&provider, &entry.model, &prompt, count, size.as_deref(), &input_images).await
        }
        ProviderType::Gemini => {
            gemini_generate_images(&provider, &entry.model, &prompt, count, &input_images).await
        }
        ProviderType::OpenRouter => {
            openrouter_generate_images(&provider, &entry.model, &prompt, count, &input_images).await
        }
        _ => Err(anyhow::anyhow!(
            "image_create does not support provider `{}` — supported: OpenAI, Gemini, OpenRouter.",
            provider.kind.as_str()
        )),
    };

    let bytes_list = match outputs {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => return Ok(error("Provider returned no image data.".to_string())),
        Err(e) => return Ok(error(format!("image_create failed: {}", e))),
    };

    let saved = save_outputs(context, "image", "png", &prompt, &bytes_list).await?;
    let cost = estimate_image_cost(&entry.model, bytes_list.len() as u32);
    deposit_cost(context, cost);
    Ok(envelope_image(
        &prompt,
        &entry.provider_key,
        &entry.model,
        &saved,
        cost,
        &image_paths_raw,
    ))
}

async fn run_video_create(
    params: Value,
    tool_use_id: &str,
    context: &ToolContext,
) -> Result<ToolOutput> {
    let entry = &context.tool_config.media.video;
    if !entry.is_configured() {
        return Ok(error(
            "video_create is not configured. Open Settings → Tools → Media to pick a provider and model.".to_string(),
        ));
    }
    let prompt = match params.get("prompt").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return Ok(error("video_create requires a non-empty `prompt`.".to_string())),
    };
    let count = clamp_count(params.get("count").and_then(|v| v.as_u64()), entry.effective_max());
    let duration = params
        .get("duration_seconds")
        .and_then(|v| v.as_u64())
        .map(|d| d.clamp(2, 12) as u32);
    let aspect = params
        .get("aspect_ratio")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // Optional first-frame image — lets the model chain
    // `image_create` → `video_create` without dropping to `animate`.
    // Same stringified-array tolerance as `image_create::image_paths`:
    // models occasionally wrap the path in `["..."]`, so coerce_single_path
    // unwraps to the first usable string.
    let image_rel = coerce_single_path(params.get("image_path"));
    let input_image_owned: Option<(String, &'static str)> = if let Some(rel) = image_rel.as_ref() {
        // Absolute paths pass through PathBuf::join unchanged, so the tool
        // accepts both project-relative and absolute filesystem paths.
        let abs_path = context.project_root.join(rel);
        if !abs_path.exists() || !abs_path.is_file() {
            return Ok(error(format!(
                "video_create: image_path `{}` not found (looked for {}). Provide either a project-relative or absolute path.",
                rel,
                abs_path.display()
            )));
        }
        let bytes = tokio::fs::read(&abs_path).await.map_err(|e| anyhow::anyhow!(e))?;
        let mime = guess_image_mime(&abs_path);
        Some((base64::engine::general_purpose::STANDARD.encode(&bytes), mime))
    } else {
        None
    };
    let input_image = input_image_owned.as_ref().map(|(b, m)| (b.as_str(), *m));

    let provider = match find_provider(&context.ai_config, &entry.provider_key) {
        Some(p) => p,
        None => return Ok(error(format!(
            "video_create: provider `{}` is not configured.",
            entry.provider_key
        ))),
    };

    context.emit_progress(tool_use_id, "Submitting video generation job…");

    let outputs = match provider.kind {
        ProviderType::OpenAi => {
            openai_generate_videos(&provider, &entry.model, &prompt, count, duration, aspect.as_deref(), input_image, tool_use_id, context).await
        }
        ProviderType::Gemini => {
            gemini_generate_videos(&provider, &entry.model, &prompt, count, duration, aspect.as_deref(), input_image, tool_use_id, context).await
        }
        ProviderType::OpenRouter => {
            openrouter_generate_videos(&provider, &entry.model, &prompt, count, duration, aspect.as_deref(), input_image, tool_use_id, context).await
        }
        _ => Err(anyhow::anyhow!(
            "video_create does not support provider `{}`.",
            provider.kind.as_str()
        )),
    };

    let bytes_list = match outputs {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => return Ok(error("Provider returned no video data.".to_string())),
        Err(e) => return Ok(error(format!("video_create failed: {}", e))),
    };

    let saved = save_outputs(context, "video", "mp4", &prompt, &bytes_list).await?;
    let cost = estimate_video_cost(&entry.model, bytes_list.len() as u32, duration);
    deposit_cost(context, cost);
    Ok(envelope(
        "video_create",
        &prompt,
        &entry.provider_key,
        &entry.model,
        &saved,
        cost,
    ))
}

async fn run_animate(
    params: Value,
    tool_use_id: &str,
    context: &ToolContext,
) -> Result<ToolOutput> {
    let entry = context.tool_config.media.effective_animate();
    if !entry.is_configured() {
        return Ok(error(
            "animate is not configured. Open Settings → Tools → Media to pick a provider and model.".to_string(),
        ));
    }
    // Same stringified-array tolerance as `image_create`: models occasionally
    // wrap the path in `["..."]`.
    let image_rel = match coerce_single_path(params.get("image_path")) {
        Some(s) => s,
        None => return Ok(error("animate requires `image_path` (project-relative).".to_string())),
    };
    let prompt = match params.get("prompt").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return Ok(error("animate requires a non-empty `prompt`.".to_string())),
    };
    let count = clamp_count(params.get("count").and_then(|v| v.as_u64()), entry.effective_max());
    let duration = params
        .get("duration_seconds")
        .and_then(|v| v.as_u64())
        .map(|d| d.clamp(2, 12) as u32);

    // Absolute paths pass through PathBuf::join unchanged, so the tool
    // accepts both project-relative and absolute filesystem paths.
    let abs_path = context.project_root.join(&image_rel);
    if !abs_path.exists() || !abs_path.is_file() {
        return Ok(error(format!(
            "animate: image not found at `{}` (looked for {}). Provide either a project-relative or absolute path.",
            image_rel,
            abs_path.display()
        )));
    }
    let bytes = tokio::fs::read(&abs_path).await.map_err(|e| anyhow::anyhow!(e))?;
    let mime = guess_image_mime(&abs_path);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let provider = match find_provider(&context.ai_config, &entry.provider_key) {
        Some(p) => p,
        None => return Ok(error(format!(
            "animate: provider `{}` is not configured.",
            entry.provider_key
        ))),
    };

    context.emit_progress(tool_use_id, "Submitting animation job…");

    let outputs = match provider.kind {
        ProviderType::OpenAi => {
            openai_generate_videos(&provider, &entry.model, &prompt, count, duration, None, Some((&b64, mime)), tool_use_id, context).await
        }
        ProviderType::Gemini => {
            gemini_generate_videos(&provider, &entry.model, &prompt, count, duration, None, Some((&b64, mime)), tool_use_id, context).await
        }
        ProviderType::OpenRouter => {
            openrouter_generate_videos(&provider, &entry.model, &prompt, count, duration, None, Some((&b64, mime)), tool_use_id, context).await
        }
        _ => Err(anyhow::anyhow!(
            "animate does not support provider `{}`.",
            provider.kind.as_str()
        )),
    };

    let bytes_list = match outputs {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => return Ok(error("Provider returned no video data.".to_string())),
        Err(e) => return Ok(error(format!("animate failed: {}", e))),
    };

    let saved = save_outputs(context, "animation", "mp4", &prompt, &bytes_list).await?;
    let cost = estimate_video_cost(&entry.model, bytes_list.len() as u32, duration);
    deposit_cost(context, cost);
    Ok(envelope_animate(
        &prompt,
        &image_rel,
        &entry.provider_key,
        &entry.model,
        &saved,
        cost,
    ))
}

// ── helpers ─────────────────────────────────────────────────────────────────

struct ResolvedProvider {
    kind: ProviderType,
    api_key: String,
    base_url: Option<String>,
}

fn find_provider(ai_config: &Arc<AiConfig>, key: &str) -> Option<ResolvedProvider> {
    let entry = ai_config.find_by_key(key)?;
    if entry.api_key.trim().is_empty() {
        return None;
    }
    Some(ResolvedProvider {
        kind: entry.provider_type.clone(),
        api_key: entry.api_key.clone(),
        base_url: entry.base_url.clone().filter(|s| !s.trim().is_empty()),
    })
}

fn clamp_count(raw: Option<u64>, max: u32) -> u32 {
    let n = raw.unwrap_or(1) as u32;
    n.max(1).min(max)
}

/// Resolve a single path parameter that the model may emit in any of these
/// shapes:
///   - native string:                 `"a.png"`
///   - native single-element array:   `["a.png"]`
///   - JSON-stringified array string: `"[\"a.png\"]"`
///   - array containing a stringified array: `["[\"a.png\"]"]`
///
/// Returns the first usable string, or `None` if nothing parses out so the
/// caller can emit its own "required" error.
fn coerce_single_path(v: Option<&Value>) -> Option<String> {
    fn unwrap_string(raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(trimmed) {
                for entry in arr {
                    if let Some(s) = entry.as_str() {
                        if let Some(unwrapped) = unwrap_string(s) {
                            return Some(unwrapped);
                        }
                    }
                }
                return None;
            }
        }
        Some(trimmed.to_string())
    }
    match v? {
        Value::String(s) => unwrap_string(s),
        Value::Array(arr) => arr.iter().find_map(|e| e.as_str().and_then(unwrap_string)),
        _ => None,
    }
}

fn guess_image_mime(path: &std::path::Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/png",
    }
}

fn error(msg: String) -> ToolOutput {
    ToolOutput {
        content: msg,
        is_error: true, attachments: Vec::new() }
}

/// JSON envelope returned to the model AND to the frontend. The chat UI
/// parses this when the tool name is `image_create` / `video_create` and
/// renders the saved files inline above the standard tool card.
fn envelope(
    tool: &str,
    prompt: &str,
    provider_key: &str,
    model: &str,
    saved: &[String],
    cost_usd: f64,
) -> ToolOutput {
    let count = saved.len();
    let kind = if tool == "image_create" { "image" } else { "video" };
    let mut human = format!(
        "Generated {} {}{} with {} ({}). Estimated cost: ${:.4}.\nPrompt: {}\nSaved:\n",
        count,
        kind,
        if count == 1 { "" } else { "s" },
        model,
        provider_key,
        cost_usd,
        prompt
    );
    for p in saved {
        human.push_str(&format!("- {}\n", p));
    }
    let payload = json!({
        "tool": tool,
        "prompt": prompt,
        "provider": provider_key,
        "model": model,
        "paths": saved,
        "cost_usd": cost_usd,
    });
    // Both human-readable + a fenced JSON block. The model reads the prose;
    // the chat-view's special renderer for media tools extracts the JSON.
    ToolOutput {
        content: format!("{}\n```media-output\n{}\n```", human, payload),
        is_error: false,
        attachments: Vec::new(),
    }
}

fn envelope_image(
    prompt: &str,
    provider_key: &str,
    model: &str,
    saved: &[String],
    cost_usd: f64,
    source_images: &[String],
) -> ToolOutput {
    let count = saved.len();
    let edited = !source_images.is_empty();
    let mut human = if edited {
        format!(
            "Edited {} source image{} into {} variant{} with {} ({}). Estimated cost: ${:.4}.\nPrompt: {}\nSources:\n",
            source_images.len(),
            if source_images.len() == 1 { "" } else { "s" },
            count,
            if count == 1 { "" } else { "s" },
            model,
            provider_key,
            cost_usd,
            prompt
        )
    } else {
        format!(
            "Generated {} image{} with {} ({}). Estimated cost: ${:.4}.\nPrompt: {}\n",
            count,
            if count == 1 { "" } else { "s" },
            model,
            provider_key,
            cost_usd,
            prompt
        )
    };
    if edited {
        for s in source_images {
            human.push_str(&format!("- {}\n", s));
        }
        human.push_str("Saved:\n");
    } else {
        human.push_str("Saved:\n");
    }
    for p in saved {
        human.push_str(&format!("- {}\n", p));
    }
    let mut payload = json!({
        "tool": "image_create",
        "prompt": prompt,
        "provider": provider_key,
        "model": model,
        "paths": saved,
        "cost_usd": cost_usd,
    });
    if edited {
        payload["source_images"] = json!(source_images);
        payload["mode"] = json!("edit");
    } else {
        payload["mode"] = json!("generate");
    }
    ToolOutput {
        content: format!("{}\n```media-output\n{}\n```", human, payload),
        is_error: false,
        attachments: Vec::new(),
    }
}

fn envelope_animate(
    prompt: &str,
    source_image: &str,
    provider_key: &str,
    model: &str,
    saved: &[String],
    cost_usd: f64,
) -> ToolOutput {
    let mut human = format!(
        "Animated {} into {} clip(s) with {} ({}). Estimated cost: ${:.4}.\nPrompt: {}\nSource: {}\nSaved:\n",
        source_image,
        saved.len(),
        model,
        provider_key,
        cost_usd,
        prompt,
        source_image
    );
    for p in saved {
        human.push_str(&format!("- {}\n", p));
    }
    let payload = json!({
        "tool": "animate",
        "prompt": prompt,
        "source_image": source_image,
        "provider": provider_key,
        "model": model,
        "paths": saved,
        "cost_usd": cost_usd,
    });
    ToolOutput {
        content: format!("{}\n```media-output\n{}\n```", human, payload),
        is_error: false,
        attachments: Vec::new(),
    }
}

/// Slug the prompt into a short filename-safe stem, then prefix with a timestamp.
fn build_filename(kind: &str, prompt: &str, idx: usize, total: usize, ext: &str) -> String {
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in prompt.chars().take(60) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "untitled" } else { slug };
    if total > 1 {
        format!("{}-{}-{}-{}.{}", ts, kind, slug, idx + 1, ext)
    } else {
        format!("{}-{}-{}.{}", ts, kind, slug, ext)
    }
}

async fn save_outputs(
    context: &ToolContext,
    kind: &str,
    ext: &str,
    prompt: &str,
    bytes_list: &[Vec<u8>],
) -> Result<Vec<String>> {
    // Images go under generated_images/, video & animation under
    // generated_videos/, both scoped by the task id so a user can find,
    // delete, or git-ignore everything tied to one chat.
    let top = if ext == "mp4" || ext == "webm" || ext == "mov" {
        GENERATED_VIDEOS_DIR
    } else {
        GENERATED_IMAGES_DIR
    };
    // Sanitise the task id — UUIDs already are filesystem-safe but
    // belt-and-braces defends against any future task-id scheme that
    // sneaks `/` or `\` characters in.
    let task_slug: String = context
        .task_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let dir = context.project_root.join(top).join(&task_slug);
    tokio::fs::create_dir_all(&dir).await.map_err(|e| anyhow::anyhow!(e))?;
    let total = bytes_list.len();
    let mut saved = Vec::with_capacity(total);
    for (idx, bytes) in bytes_list.iter().enumerate() {
        let name = build_filename(kind, prompt, idx, total, ext);
        let path: PathBuf = dir.join(&name);
        tokio::fs::write(&path, bytes).await.map_err(|e| anyhow::anyhow!(e))?;
        // Return project-relative for the chat UI to resolve.
        saved.push(format!("{}/{}/{}", top, task_slug, name));
    }
    Ok(saved)
}

// ── OpenAI image generation ─────────────────────────────────────────────────

async fn openai_generate_images(
    provider: &ResolvedProvider,
    model: &str,
    prompt: &str,
    count: u32,
    size: Option<&str>,
    input_images: &[(&str, &str)],
) -> Result<Vec<Vec<u8>>> {
    let base = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "https://api.openai.com".to_string());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()?;

    // Image-to-image / editing path uses /v1/images/edits with multipart
    // form-data. gpt-image-1 (and successors) accept multiple `image[]`
    // fields; dall-e-2 supports a single image plus optional mask. We don't
    // accept a mask parameter — modern image models can localise edits from
    // the prompt alone, which is the user-facing contract for this tool.
    if !input_images.is_empty() {
        let url = format!("{}/v1/images/edits", base.trim_end_matches('/'));
        let size_value = size.unwrap_or("1024x1024");
        let mut form = reqwest::multipart::Form::new()
            .text("model", model.to_string())
            .text("prompt", prompt.to_string())
            .text("n", count.to_string())
            .text("size", size_value.to_string());
        // dall-e-* needs response_format=b64_json; gpt-image-* always
        // returns b64 and rejects the field, so only set it for dall-e.
        if model.starts_with("dall-e") {
            form = form.text("response_format", "b64_json".to_string());
        }
        // Multi-image: gpt-image-* accepts `image[]` repeated. Single-image
        // models will just use the first attachment.
        let field_name = if input_images.len() > 1 { "image[]" } else { "image" };
        for (idx, (b64, mime)) in input_images.iter().enumerate() {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| anyhow::anyhow!("OpenAI input image decode: {}", e))?;
            let ext = match *mime {
                "image/jpeg" => "jpg",
                "image/webp" => "webp",
                "image/gif" => "gif",
                _ => "png",
            };
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(format!("source-{}.{}", idx, ext))
                .mime_str(mime)
                .map_err(|e| anyhow::anyhow!("OpenAI image mime: {}", e))?;
            form = form.part(field_name, part);
        }

        let resp = client
            .post(&url)
            .bearer_auth(&provider.api_key)
            .multipart(form)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!("OpenAI edits {} → {}", status, truncate(&text, 600)));
        }
        let v: Value = serde_json::from_str(&text)?;
        let mut out = Vec::new();
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            for item in arr {
                if let Some(b64) = item.get("b64_json").and_then(|s| s.as_str()) {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|e| anyhow::anyhow!("OpenAI edits b64 decode: {}", e))?;
                    out.push(bytes);
                } else if let Some(url) = item.get("url").and_then(|s| s.as_str()) {
                    let bytes = client.get(url).send().await?.bytes().await?.to_vec();
                    out.push(bytes);
                }
            }
        }
        return Ok(out);
    }

    // Text-to-image path.
    let url = format!("{}/v1/images/generations", base.trim_end_matches('/'));
    let size_value = size.unwrap_or("1024x1024");
    let mut body = json!({
        "model": model,
        "prompt": prompt,
        "n": count,
        "size": size_value,
    });
    if model.starts_with("dall-e") {
        body["response_format"] = json!("b64_json");
    }

    let resp = client
        .post(&url)
        .bearer_auth(&provider.api_key)
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow::anyhow!("OpenAI {} → {}", status, truncate(&text, 600)));
    }
    let v: Value = serde_json::from_str(&text)?;
    let mut out = Vec::new();
    if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
        for item in arr {
            if let Some(b64) = item.get("b64_json").and_then(|s| s.as_str()) {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| anyhow::anyhow!("OpenAI b64 decode: {}", e))?;
                out.push(bytes);
            } else if let Some(url) = item.get("url").and_then(|s| s.as_str()) {
                let bytes = client.get(url).send().await?.bytes().await?.to_vec();
                out.push(bytes);
            }
        }
    }
    Ok(out)
}

// ── Gemini image generation ─────────────────────────────────────────────────

async fn gemini_generate_images(
    provider: &ResolvedProvider,
    model: &str,
    prompt: &str,
    count: u32,
    input_images: &[(&str, &str)],
) -> Result<Vec<Vec<u8>>> {
    // Gemini 2.5 flash-image returns inline image bytes via generateContent.
    // The model emits N images when asked; we issue `count` separate calls
    // when count > 1 because the per-call multi-image control is awkward.
    // For image-to-image / editing, the same endpoint handles it — we just
    // attach `inlineData` parts for each source image alongside the text
    // prompt. Gemini Nano Banana supports multi-image composition naturally.
    let base = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());
    let url = format!(
        "{}/v1beta/models/{}:generateContent?key={}",
        base.trim_end_matches('/'),
        model,
        urlencode(&provider.api_key)
    );

    let mut parts: Vec<Value> = Vec::new();
    for (b64, mime) in input_images {
        parts.push(json!({
            "inlineData": {
                "mimeType": *mime,
                "data": *b64,
            }
        }));
    }
    parts.push(json!({ "text": prompt }));

    let body = json!({
        "contents": [{ "parts": parts }],
        "generationConfig": {
            "responseModalities": ["IMAGE"]
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()?;
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let resp = client.post(&url).json(&body).send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!("Gemini {} → {}", status, truncate(&text, 600)));
        }
        let v: Value = serde_json::from_str(&text)?;
        if let Some(cands) = v.get("candidates").and_then(|c| c.as_array()) {
            for cand in cands {
                if let Some(parts) = cand
                    .get("content")
                    .and_then(|c| c.get("parts"))
                    .and_then(|p| p.as_array())
                {
                    for part in parts {
                        if let Some(b64) = part
                            .get("inlineData")
                            .and_then(|d| d.get("data"))
                            .and_then(|s| s.as_str())
                        {
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(b64)
                                .map_err(|e| anyhow::anyhow!("Gemini b64 decode: {}", e))?;
                            out.push(bytes);
                            break;
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

// ── OpenRouter image generation ─────────────────────────────────────────────

async fn openrouter_generate_images(
    provider: &ResolvedProvider,
    model: &str,
    prompt: &str,
    count: u32,
    input_images: &[(&str, &str)],
) -> Result<Vec<Vec<u8>>> {
    // OpenRouter uses /v1/chat/completions with `modalities: ["image", "text"]`.
    // Images come back as data URLs in `choices[0].message.images[].image_url.url`.
    // For image-to-image / editing we send the message `content` as an array
    // of `image_url` + `text` parts, matching the OpenAI-compatible vision
    // shape that the underlying image models (Gemini Nano Banana via
    // OpenRouter, Seedream, GPT image families) all accept.
    let base = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "https://openrouter.ai/api".to_string());
    let url = format!("{}/v1/chat/completions", base.trim_end_matches('/'));

    let content_value: Value = if input_images.is_empty() {
        json!(prompt)
    } else {
        let mut items: Vec<Value> = Vec::with_capacity(input_images.len() + 1);
        for (b64, mime) in input_images {
            items.push(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", mime, b64) }
            }));
        }
        items.push(json!({ "type": "text", "text": prompt }));
        Value::Array(items)
    };

    let body = json!({
        "model": model,
        "modalities": ["image", "text"],
        "messages": [{ "role": "user", "content": content_value }],
        "n": count,
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()?;
    let resp = client
        .post(&url)
        .bearer_auth(&provider.api_key)
        .header("HTTP-Referer", "https://github.com/avijitbhuin21/Rustic")
        .header("X-Title", "Rustic")
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow::anyhow!("OpenRouter {} → {}", status, truncate(&text, 600)));
    }
    let v: Value = serde_json::from_str(&text)?;
    let mut out = Vec::new();
    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
        for choice in choices {
            let imgs = choice
                .get("message")
                .and_then(|m| m.get("images"))
                .and_then(|i| i.as_array());
            if let Some(imgs) = imgs {
                for img in imgs {
                    if let Some(url) = img
                        .get("image_url")
                        .and_then(|u| u.get("url"))
                        .and_then(|s| s.as_str())
                    {
                        if let Some(bytes) = decode_data_url(url) {
                            out.push(bytes);
                        } else {
                            // Plain URL — fetch
                            let bytes = client.get(url).send().await?.bytes().await?.to_vec();
                            out.push(bytes);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

fn decode_data_url(url: &str) -> Option<Vec<u8>> {
    if !url.starts_with("data:") {
        return None;
    }
    let comma = url.find(',')?;
    let meta = &url[..comma];
    let body = &url[comma + 1..];
    if meta.contains("base64") {
        base64::engine::general_purpose::STANDARD.decode(body).ok()
    } else {
        Some(body.as_bytes().to_vec())
    }
}

// ── OpenAI video generation (Sora-style /v1/videos with polling) ────────────

async fn openai_generate_videos(
    provider: &ResolvedProvider,
    model: &str,
    prompt: &str,
    count: u32,
    duration: Option<u32>,
    aspect: Option<&str>,
    input_image: Option<(&str, &str)>, // (b64, mime)
    tool_use_id: &str,
    context: &ToolContext,
) -> Result<Vec<Vec<u8>>> {
    let base = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "https://api.openai.com".to_string());
    let create_url = format!("{}/v1/videos", base.trim_end_matches('/'));

    let mut out = Vec::with_capacity(count as usize);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    for idx in 0..count {
        // Submit
        let mut body = json!({
            "model": model,
            "prompt": prompt,
        });
        if let Some(d) = duration {
            body["seconds"] = json!(d.to_string());
        }
        if let Some(a) = aspect {
            // OpenAI uses `size` like "1280x720"; aspect remap is best-effort.
            body["size"] = match a {
                "16:9" => json!("1280x720"),
                "9:16" => json!("720x1280"),
                "1:1" => json!("720x720"),
                _ => json!(a),
            };
        }
        if let Some((b64, mime)) = input_image {
            // OpenAI Sora accepts an `input_reference` as a file id or data URL.
            body["input_reference"] = json!(format!("data:{};base64,{}", mime, b64));
        }

        let resp = client
            .post(&create_url)
            .bearer_auth(&provider.api_key)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!("OpenAI videos {} → {}", status, truncate(&text, 600)));
        }
        let job: Value = serde_json::from_str(&text)?;
        let job_id = job
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("OpenAI videos: missing job id"))?
            .to_string();

        // Poll
        let status_url = format!("{}/v1/videos/{}", base.trim_end_matches('/'), job_id);
        let mut waited = 0u64;
        let deadline_secs = 600u64; // 10 minutes cap
        let poll_secs = 8u64;
        loop {
            if check_cancel(context) {
                return Err(anyhow::anyhow!("cancelled"));
            }
            tokio::time::sleep(std::time::Duration::from_secs(poll_secs)).await;
            waited += poll_secs;
            context.emit_progress(
                tool_use_id,
                &format!("Generating video {}/{} ({}s elapsed)…", idx + 1, count, waited),
            );
            let r = client
                .get(&status_url)
                .bearer_auth(&provider.api_key)
                .send()
                .await?;
            let s = r.status();
            let t = r.text().await.unwrap_or_default();
            if !s.is_success() {
                return Err(anyhow::anyhow!("OpenAI videos poll {} → {}", s, truncate(&t, 400)));
            }
            let j: Value = serde_json::from_str(&t)?;
            let status_str = j.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if status_str == "completed" {
                // Download
                let dl_url = format!(
                    "{}/v1/videos/{}/content",
                    base.trim_end_matches('/'),
                    job_id
                );
                let dl = client
                    .get(&dl_url)
                    .bearer_auth(&provider.api_key)
                    .send()
                    .await?;
                if !dl.status().is_success() {
                    return Err(anyhow::anyhow!(
                        "OpenAI videos download {}",
                        dl.status()
                    ));
                }
                out.push(dl.bytes().await?.to_vec());
                break;
            } else if status_str == "failed" || status_str == "cancelled" {
                let msg = j
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("(no message)");
                return Err(anyhow::anyhow!("OpenAI video job {}: {}", status_str, msg));
            }
            if waited >= deadline_secs {
                return Err(anyhow::anyhow!("OpenAI video timed out after {}s", waited));
            }
        }
    }
    Ok(out)
}

// ── Gemini video generation (Veo predictLongRunning + operations poll) ──────

async fn gemini_generate_videos(
    provider: &ResolvedProvider,
    model: &str,
    prompt: &str,
    count: u32,
    duration: Option<u32>,
    aspect: Option<&str>,
    input_image: Option<(&str, &str)>, // (b64, mime)
    tool_use_id: &str,
    context: &ToolContext,
) -> Result<Vec<Vec<u8>>> {
    let base = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());
    let predict_url = format!(
        "{}/v1beta/models/{}:predictLongRunning?key={}",
        base.trim_end_matches('/'),
        model,
        urlencode(&provider.api_key)
    );

    let mut instance = json!({ "prompt": prompt });
    if let Some((b64, mime)) = input_image {
        instance["image"] = json!({
            "bytesBase64Encoded": b64,
            "mimeType": mime,
        });
    }

    let mut parameters = json!({ "sampleCount": count });
    if let Some(d) = duration {
        parameters["durationSeconds"] = json!(d);
    }
    if let Some(a) = aspect {
        parameters["aspectRatio"] = json!(a);
    }

    let body = json!({
        "instances": [instance],
        "parameters": parameters,
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let resp = client.post(&predict_url).json(&body).send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow::anyhow!("Veo {} → {}", status, truncate(&text, 600)));
    }
    let op: Value = serde_json::from_str(&text)?;
    let op_name = op
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Veo: missing operation name"))?
        .to_string();

    // Poll. Operations resource lives under the same base.
    let op_url = format!(
        "{}/v1beta/{}?key={}",
        base.trim_end_matches('/'),
        op_name,
        urlencode(&provider.api_key)
    );

    let mut waited = 0u64;
    let deadline = 600u64;
    let poll = 10u64;
    let final_op = loop {
        if check_cancel(context) {
            return Err(anyhow::anyhow!("cancelled"));
        }
        tokio::time::sleep(std::time::Duration::from_secs(poll)).await;
        waited += poll;
        context.emit_progress(
            tool_use_id,
            &format!("Veo generating ({}s elapsed)…", waited),
        );
        let r = client.get(&op_url).send().await?;
        let s = r.status();
        let t = r.text().await.unwrap_or_default();
        if !s.is_success() {
            return Err(anyhow::anyhow!("Veo poll {} → {}", s, truncate(&t, 400)));
        }
        let j: Value = serde_json::from_str(&t)?;
        if j.get("done").and_then(|v| v.as_bool()).unwrap_or(false) {
            break j;
        }
        if waited >= deadline {
            return Err(anyhow::anyhow!("Veo timed out after {}s", waited));
        }
    };

    // Response contains generatedSamples[].video.uri OR inline videoBytes.
    let mut out = Vec::new();
    let videos = final_op
        .get("response")
        .and_then(|r| r.get("generateVideoResponse"))
        .and_then(|g| g.get("generatedSamples"))
        .and_then(|s| s.as_array())
        .cloned()
        .or_else(|| {
            final_op
                .get("response")
                .and_then(|r| r.get("generatedSamples"))
                .and_then(|s| s.as_array())
                .cloned()
        })
        .unwrap_or_default();

    for v in videos {
        if let Some(uri) = v
            .get("video")
            .and_then(|vv| vv.get("uri"))
            .and_then(|s| s.as_str())
        {
            // The URI requires the same API key as a query parameter for download.
            let dl = if uri.contains("key=") {
                client.get(uri).send().await?
            } else {
                let sep = if uri.contains('?') { '&' } else { '?' };
                client
                    .get(format!("{}{}key={}", uri, sep, urlencode(&provider.api_key)))
                    .send()
                    .await?
            };
            if !dl.status().is_success() {
                return Err(anyhow::anyhow!("Veo download {}", dl.status()));
            }
            out.push(dl.bytes().await?.to_vec());
        } else if let Some(b64) = v
            .get("video")
            .and_then(|vv| vv.get("videoBytes"))
            .and_then(|s| s.as_str())
        {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| anyhow::anyhow!("Veo b64 decode: {}", e))?;
            out.push(bytes);
        }
    }
    Ok(out)
}

// ── OpenRouter video generation (async /v1/videos with status polling) ─────

async fn openrouter_generate_videos(
    provider: &ResolvedProvider,
    model: &str,
    prompt: &str,
    count: u32,
    duration: Option<u32>,
    aspect: Option<&str>,
    input_image: Option<(&str, &str)>, // (b64, mime)
    tool_use_id: &str,
    context: &ToolContext,
) -> Result<Vec<Vec<u8>>> {
    // OpenRouter exposes video generation at /api/v1/videos. Same job
    // lifecycle as OpenAI's videos endpoint: POST returns a job id, GET
    // /v1/videos/{id} reports status, GET /v1/videos/{id}/content?index=N
    // returns the bytes. Unlike chat-completions we cannot batch with `n`,
    // so we submit `count` independent jobs and poll each.
    let base = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "https://openrouter.ai/api".to_string());
    let create_url = format!("{}/v1/videos", base.trim_end_matches('/'));

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    let mut out = Vec::with_capacity(count as usize);
    for idx in 0..count {
        let mut body = json!({
            "model": model,
            "prompt": prompt,
        });
        if let Some(d) = duration {
            body["duration"] = json!(d);
        }
        if let Some(a) = aspect {
            body["aspect_ratio"] = json!(a);
        }
        if let Some((b64, mime)) = input_image {
            body["frame_images"] = json!([{
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", mime, b64) },
                "frame_type": "first_frame",
            }]);
        }

        let resp = client
            .post(&create_url)
            .bearer_auth(&provider.api_key)
            .header("HTTP-Referer", "https://github.com/avijitbhuin21/Rustic")
            .header("X-Title", "Rustic")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "OpenRouter videos {} → {}",
                status,
                truncate(&text, 600)
            ));
        }
        let job: Value = serde_json::from_str(&text)?;
        let job_id = job
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("OpenRouter videos: missing job id"))?
            .to_string();

        // Poll. OpenRouter recommends 30s intervals; we use 12s so the UI
        // progress line refreshes more often.
        let status_url = format!(
            "{}/v1/videos/{}",
            base.trim_end_matches('/'),
            job_id
        );
        let mut waited = 0u64;
        let deadline_secs = 900u64; // 15 min cap (OpenRouter video can be slow)
        let poll_secs = 12u64;
        let final_job = loop {
            if check_cancel(context) {
                return Err(anyhow::anyhow!("cancelled"));
            }
            tokio::time::sleep(std::time::Duration::from_secs(poll_secs)).await;
            waited += poll_secs;
            context.emit_progress(
                tool_use_id,
                &format!(
                    "OpenRouter video {}/{} ({}s elapsed)…",
                    idx + 1,
                    count,
                    waited
                ),
            );
            let r = client
                .get(&status_url)
                .bearer_auth(&provider.api_key)
                .send()
                .await?;
            let s = r.status();
            let t = r.text().await.unwrap_or_default();
            if !s.is_success() {
                return Err(anyhow::anyhow!(
                    "OpenRouter videos poll {} → {}",
                    s,
                    truncate(&t, 400)
                ));
            }
            let j: Value = serde_json::from_str(&t)?;
            let st = j.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if st == "completed" || st == "succeeded" {
                break j;
            }
            if st == "failed" || st == "cancelled" || st == "error" {
                let msg = j
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .or_else(|| j.get("error"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("(no message)");
                return Err(anyhow::anyhow!("OpenRouter video {}: {}", st, msg));
            }
            if waited >= deadline_secs {
                return Err(anyhow::anyhow!("OpenRouter video timed out after {}s", waited));
            }
        };

        // Download. Prefer the documented /content?index=0 endpoint; fall
        // back to unsigned_urls[0] if the server points elsewhere.
        let dl_url = format!(
            "{}/v1/videos/{}/content?index=0",
            base.trim_end_matches('/'),
            job_id
        );
        let dl = client
            .get(&dl_url)
            .bearer_auth(&provider.api_key)
            .send()
            .await?;
        if dl.status().is_success() {
            out.push(dl.bytes().await?.to_vec());
            continue;
        }
        // Fallback to unsigned_urls[0] if /content failed.
        if let Some(url) = final_job
            .get("unsigned_urls")
            .and_then(|u| u.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
        {
            let r = client.get(url).send().await?;
            if !r.status().is_success() {
                return Err(anyhow::anyhow!("OpenRouter video download {}", r.status()));
            }
            out.push(r.bytes().await?.to_vec());
        } else {
            return Err(anyhow::anyhow!(
                "OpenRouter videos: completed job has no downloadable url"
            ));
        }
    }
    Ok(out)
}

// ── shared util ─────────────────────────────────────────────────────────────

fn check_cancel(context: &ToolContext) -> bool {
    context
        .cancel_token
        .as_ref()
        .map(|t| t.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(false)
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    format!("{}…", &s[..end])
}
