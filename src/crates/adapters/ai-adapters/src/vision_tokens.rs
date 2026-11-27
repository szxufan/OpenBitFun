//! Geometry-based estimates and resizing for inline vision inputs.

use anyhow::{anyhow, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use image::codecs::{gif::GifDecoder, jpeg::JpegEncoder, png::PngDecoder, webp::WebPDecoder};
use image::{
    imageops::FilterType, AnimationDecoder, DynamicImage, GenericImageView, ImageDecoder,
    ImageFormat, ImageReader, Limits,
};
use std::io::Cursor;

mod openai_patches;
use openai_patches::OpenAiPatches;

/// Size reserved for path-only history, opaque URLs or unreadable headers.
pub const UNKNOWN_IMAGE_DIMENSIONS: (u32, u32) = (2048, 2048);

fn grid(width: u32, height: u32, edge: u32) -> u64 {
    u64::from(width.div_ceil(edge)) * u64::from(height.div_ceil(edge))
}

/// Model-neutral estimate used before the runtime selects a provider.
pub fn estimate_image_tokens(width: u32, height: u32) -> usize {
    usize::try_from(50 + 200 * grid(width, height, 512)).unwrap_or(usize::MAX)
}

/// Estimates for known vision processors. A custom model ID retains the
/// model-neutral tile estimate when its processor is unknown.
#[derive(Clone, Copy)]
pub(crate) enum VisionTokenPolicy {
    Generic,
    OpenAiTiles { base: u64, tile: u64 },
    OpenAiPatches(OpenAiPatches),
    Fixed(u64),
    Gemini(u64),
    Claude { max_edge: u32, max_tokens: u64 },
    Patches { edge: u32, max: u64, percent: u64 },
}

impl VisionTokenPolicy {
    pub(crate) fn for_model(model: &str) -> Self {
        let name = model
            .rsplit('/')
            .next()
            .unwrap_or(model)
            .to_ascii_lowercase();
        // OpenAI snapshot names end in a date, e.g. gpt-4o-2024-08-06.
        let family = name.split_once("-20").map_or(name.as_str(), |(id, _)| id);
        if let Some(patches) = OpenAiPatches::for_model(family) {
            return Self::OpenAiPatches(patches);
        }
        match family {
            // https://developers.openai.com/api/docs/guides/images-vision
            "gpt-4.1" | "gpt-4o" => Self::OpenAiTiles {
                base: 85,
                tile: 170,
            },
            "gpt-4o-mini" => Self::OpenAiTiles {
                base: 2833,
                tile: 5667,
            },
            "gpt-5" | "gpt-5.1" => Self::OpenAiTiles {
                base: 70,
                tile: 140,
            },
            "o1" | "o1-pro" | "o3" => Self::OpenAiTiles {
                base: 75,
                tile: 150,
            },
            "gpt-4.1-nano" | "o4-mini" => {
                let percent = if family == "gpt-4.1-nano" { 246 } else { 172 };
                Self::Patches {
                    edge: 32,
                    max: 1536,
                    percent,
                }
            }
            // A patch upper bound before any deployment-specific resize.
            // https://huggingface.co/Qwen/Qwen2.5-VL-7B-Instruct
            id if id.starts_with("qwen2.5-vl") || id.starts_with("qwen2-vl") => Self::Patches {
                edge: 28,
                max: u64::MAX,
                percent: 100,
            },
            // https://ai.google.dev/gemini-api/docs/media-resolution
            id if id.starts_with("gemini-3.")
                || id.starts_with("gemini-3-")
                || id.starts_with("gemini-4.")
                || id.starts_with("gemini-4-") =>
            {
                Self::Gemini(1120)
            }
            // https://platform.claude.com/docs/en/build-with-claude/vision
            id if id.starts_with("claude-") => {
                let id = id.strip_suffix("-latest").unwrap_or(id);
                let standard = id.starts_with("claude-3-")
                    || matches!(
                        id,
                        "claude-opus-4"
                            | "claude-sonnet-4"
                            | "claude-opus-4-1"
                            | "claude-opus-4-5"
                            | "claude-sonnet-4-5"
                            | "claude-haiku-4-5"
                            | "claude-opus-4-6"
                            | "claude-sonnet-4-6"
                    );
                let (max_edge, max_tokens) = if standard { (1568, 1568) } else { (2576, 4784) };
                Self::Claude {
                    max_edge,
                    max_tokens,
                }
            }
            _ => Self::Generic,
        }
    }

    pub(crate) fn with_detail(self, detail: Option<&str>) -> Self {
        match (self, detail) {
            (Self::OpenAiPatches(patches), detail) => {
                Self::OpenAiPatches(patches.with_detail(detail))
            }
            (Self::OpenAiTiles { base, .. }, Some("low")) => Self::Fixed(base),
            _ => self,
        }
    }

    pub(crate) fn with_gemini_media_resolution(self, resolution: Option<&str>) -> Self {
        if !matches!(self, Self::Gemini(_)) {
            return self;
        }
        match resolution {
            Some("MEDIA_RESOLUTION_LOW") => Self::Gemini(280),
            Some("MEDIA_RESOLUTION_MEDIUM") => Self::Gemini(560),
            Some("MEDIA_RESOLUTION_HIGH" | "MEDIA_RESOLUTION_UNSPECIFIED") => Self::Gemini(1120),
            Some("MEDIA_RESOLUTION_ULTRA_HIGH") => Self::Gemini(2240),
            _ => self,
        }
    }

    pub(crate) fn estimate(self, width: u32, height: u32) -> usize {
        let cost = match self {
            Self::Generic => return estimate_image_tokens(width, height),
            Self::OpenAiPatches(patches) => patches.estimate(width, height),
            Self::Fixed(tokens) | Self::Gemini(tokens) => tokens,
            Self::Claude {
                max_edge,
                max_tokens,
            } => {
                let longest = width.max(height);
                let (w, h) = scale_long_edge(width, height, longest.min(max_edge));
                grid(w, h, 28).min(max_tokens)
            }

            Self::Patches { edge, max, percent } => {
                (grid(width, height, edge).min(max) * percent).div_ceil(100)
            }
            Self::OpenAiTiles { base, tile } => {
                let scale = (2048.0 / f64::from(width.max(height)))
                    .min(768.0 / f64::from(width.min(height)))
                    .min(1.0);
                let resize = |side| ((f64::from(side) * scale) as u32).max(1);
                base + tile * grid(resize(width), resize(height), 512)
            }
        };
        usize::try_from(cost).unwrap_or(usize::MAX)
    }
}

/// Read header metadata without decoding a pixel buffer. Most screenshots and
/// photos expose dimensions near the start, even when their payload is large.
pub fn inline_image_dimensions(mime_type: &str, data_base64: &str) -> Option<(u32, u32)> {
    let format = ImageFormat::from_mime_type(mime_type)?;
    let dimensions = |bytes: &[u8]| {
        ImageReader::with_format(Cursor::new(bytes), format)
            .into_dimensions()
            .ok()
            .filter(|&(w, h)| w > 0 && h > 0)
    };
    let encoded = data_base64.as_bytes();
    const PREFIX: usize = 4096; // Complete base64 quartets.
    if encoded.len() > PREFIX {
        if let Some(size) = dimensions(&STANDARD.decode(&encoded[..PREFIX]).ok()?) {
            return Some(size);
        }
    }
    dimensions(&STANDARD.decode(encoded).ok()?)
}

pub(crate) struct ResizedInlineImage {
    pub mime_type: String,
    pub data_base64: String,
    pub width: u32,
    pub height: u32,
}

fn scale_long_edge(width: u32, height: u32, edge: u32) -> (u32, u32) {
    let longest = u64::from(width.max(height).max(1));
    let scale = |side| ((u64::from(side) * u64::from(edge) + longest / 2) / longest).max(1) as u32;
    (scale(width), scale(height))
}

impl VisionTokenPolicy {
    /// Reserve a preview with a 512-pixel long edge, or the entire smaller input.
    pub(crate) fn min_readable_tokens(self, width: u32, height: u32) -> usize {
        let (w, h) = scale_long_edge(width, height, width.max(height).min(512));
        self.estimate(w, h)
    }

    /// Find the largest proportional pixel dimensions that fit before resampling.
    fn fit_dimensions(self, width: u32, height: u32, budget: usize) -> Result<(u32, u32)> {
        let minimum = self.min_readable_tokens(width, height);
        ensure!(
            budget >= minimum,
            "A readable image preview needs at least {minimum} estimated tokens; {budget} available. Crop the image or shorten the context."
        );
        let longest = width.max(height);
        let (mut lo, mut hi) = (longest.min(512), longest);
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            let (w, h) = scale_long_edge(width, height, mid);
            if self.estimate(w, h) <= budget {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        Ok(scale_long_edge(width, height, lo))
    }
}

/// Avoid silently substituting one animation frame for an entire visual input.
fn decode_still(
    bytes: &[u8],
    format: ImageFormat,
) -> Result<(DynamicImage, image::metadata::Orientation)> {
    let input = Cursor::new(bytes);
    let mut decoder: Box<dyn ImageDecoder + '_> = match format {
        ImageFormat::Png => {
            let png = PngDecoder::with_limits(input, Limits::default())?;
            ensure!(
                !png.is_apng()?,
                "Cannot resize an animated PNG as a still image"
            );
            Box::new(png)
        }
        ImageFormat::WebP => {
            let webp = WebPDecoder::new(input)?;
            ensure!(
                !webp.has_animation(),
                "Cannot resize an animated WebP as a still image"
            );
            Box::new(webp)
        }
        ImageFormat::Gif => {
            let mut gif = GifDecoder::new(input)?;
            gif.set_limits(Limits::default())?;
            let mut frames = gif.into_frames();
            let first = frames.next().context("GIF has no frames")??;
            ensure!(
                frames.next().transpose()?.is_none(),
                "Cannot resize an animated GIF as a still image"
            );
            return Ok((
                first.into_buffer().into(),
                image::metadata::Orientation::NoTransforms,
            ));
        }
        ImageFormat::Jpeg | ImageFormat::Bmp => {
            Box::new(ImageReader::with_format(input, format).into_decoder()?)
        }
        _ => anyhow::bail!("Unsupported inline image format {format:?}"),
    };
    let orientation = decoder.orientation()?;
    let mut limits = Limits::default();
    limits.reserve(decoder.total_bytes())?;
    decoder.set_limits(limits)?;
    Ok((DynamicImage::from_decoder(decoder)?, orientation))
}

impl VisionTokenPolicy {
    /// Resize only the outgoing image, retaining text edges and transparency.
    pub(crate) fn resize(
        self,
        mime_type: &str,
        data_base64: &str,
        target_tokens: usize,
    ) -> Result<ResizedInlineImage> {
        let format = ImageFormat::from_mime_type(mime_type)
            .ok_or_else(|| anyhow!("Unsupported inline image type {mime_type}"))?;
        let (width, height) = inline_image_dimensions(mime_type, data_base64)
            .context("Cannot read inline image dimensions")?;
        if self.estimate(width, height) <= target_tokens {
            return Ok(ResizedInlineImage {
                mime_type: mime_type.to_owned(),
                data_base64: data_base64.to_owned(),
                width,
                height,
            });
        }
        let (w, h) = self.fit_dimensions(width, height, target_tokens)?;
        let bytes = STANDARD
            .decode(data_base64)
            .context("Invalid inline image base64")?;
        let (source, orientation) = decode_still(&bytes, format)?;
        let mut resized = source.resize_exact(w, h, FilterType::Lanczos3);
        resized.apply_orientation(orientation);
        let (width, height) = resized.dimensions();
        let mut output = Cursor::new(Vec::new());
        let output_format = match format {
            ImageFormat::Jpeg => {
                JpegEncoder::new_with_quality(&mut output, 90).encode_image(&resized)?;
                ImageFormat::Jpeg
            }
            _ => {
                let format = if format == ImageFormat::WebP {
                    ImageFormat::WebP
                } else {
                    ImageFormat::Png
                };
                resized.write_to(&mut output, format)?;
                format
            }
        };
        Ok(ResizedInlineImage {
            mime_type: output_format.to_mime_type().to_owned(),
            data_base64: STANDARD.encode(output.into_inner()),
            width,
            height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_model_and_image_detail_determine_the_visual_estimate() {
        let tiles = VisionTokenPolicy::for_model("openai/gpt-4o-2024-08-06");
        assert_eq!(tiles.estimate(2048, 2048), 765);
        assert_eq!(tiles.with_detail(Some("low")).estimate(2048, 2048), 85);
        assert_eq!(
            VisionTokenPolicy::for_model("gpt-4o-mini").estimate(512, 512),
            8500
        );
        assert_eq!(
            VisionTokenPolicy::for_model("gpt-4.1-mini").estimate(1024, 1024),
            1659
        );
        assert_eq!(
            VisionTokenPolicy::for_model("Qwen/Qwen2.5-VL-7B-Instruct").estimate(280, 560),
            200
        );
        assert_eq!(
            VisionTokenPolicy::for_model("custom").estimate(2048, 2048),
            3250
        );
    }

    #[test]
    fn openai_patch_models_apply_their_own_detail_defaults_and_patch_limits() {
        let astra = VisionTokenPolicy::for_model("openai/gpt-6-astra-2026-01-01");
        assert_eq!(astra.estimate(2048, 2048), 4916);
        assert_eq!(astra.with_detail(Some("auto")).estimate(2048, 2048), 4916);
        assert_eq!(astra.with_detail(Some("low")).estimate(2048, 2048), 308);
        assert_eq!(astra.with_detail(Some("high")).estimate(2048, 2048), 3000);
        assert_eq!(astra.with_detail(Some("high")).estimate(4096, 512), 2458);

        for name in ["gpt-5.4", "gpt-5.4-mini"] {
            let policy = VisionTokenPolicy::for_model(name);
            assert_eq!(policy.estimate(2048, 2048), 3000);
            assert_eq!(policy.with_detail(Some("low")).estimate(2048, 2048), 4916);
            assert_eq!(
                policy.with_detail(Some("original")).estimate(4096, 4096),
                12_000
            );
            // 2048x1536 exceeds 2500 patches. The scaled grid is 57x43.
            assert_eq!(policy.estimate(2048, 1536), 2942);
        }
        let mini = VisionTokenPolicy::for_model("gpt-4.1-mini-2025-04-14");
        for detail in ["low", "high", "auto"] {
            assert_eq!(mini.with_detail(Some(detail)).estimate(4096, 4096), 6636);
        }
    }

    #[test]
    fn gemini_resolution_and_claude_tiers_bound_visual_tokens() {
        for model in ["gemini-3.6-flash", "gemini-4-hydrogen"] {
            let policy = VisionTokenPolicy::for_model(model);
            assert_eq!(policy.estimate(32, 32), 1120);
            for (level, expected) in [
                ("MEDIA_RESOLUTION_LOW", 280),
                ("MEDIA_RESOLUTION_MEDIUM", 560),
                ("MEDIA_RESOLUTION_HIGH", 1120),
                ("MEDIA_RESOLUTION_ULTRA_HIGH", 2240),
            ] {
                assert_eq!(
                    policy
                        .with_gemini_media_resolution(Some(level))
                        .estimate(2048, 1024),
                    expected
                );
            }
        }
        let standard = VisionTokenPolicy::for_model("claude-haiku-4-5-20251001");
        assert_eq!(standard.estimate(4000, 4000), 1568);
        assert_eq!(standard.estimate(8192, 128), 56);
        let high = VisionTokenPolicy::for_model("claude-sonnet-5");
        assert_eq!(high.estimate(3840, 2160), 4784);
        assert_eq!(high.estimate(1000, 1000), 1296);
    }

    #[test]
    fn oversized_screenshot_fits_at_largest_resolution_and_retains_alpha() {
        let source = DynamicImage::new_rgba8(2048, 1024);
        let mut bytes = Cursor::new(Vec::new());
        source.write_to(&mut bytes, ImageFormat::Png).unwrap();
        let data = STANDARD.encode(bytes.into_inner());
        assert_eq!(
            inline_image_dimensions("image/png", &data),
            Some((2048, 1024))
        );
        let budget = estimate_image_tokens(1024, 512);
        let projected = VisionTokenPolicy::Generic
            .resize("image/png", &data, budget)
            .unwrap();
        assert!(estimate_image_tokens(projected.width, projected.height) <= budget);
        assert!(
            estimate_image_tokens(projected.width + 1, (projected.width + 1).div_ceil(2)) > budget
        );
        let decoded =
            image::load_from_memory(&STANDARD.decode(projected.data_base64).unwrap()).unwrap();
        assert_eq!(decoded.dimensions(), (projected.width, projected.height));
        assert_eq!(decoded.to_rgba8().get_pixel(0, 0)[3], 0);
        let original = VisionTokenPolicy::Generic
            .resize("image/png", &data, usize::MAX)
            .unwrap();
        assert_eq!(original.data_base64, data);
    }

    #[test]
    fn invalid_input_and_infeasible_budgets_are_explicit() {
        assert!(inline_image_dimensions("image/png", "@@").is_none());
        assert!(VisionTokenPolicy::Generic
            .resize("image/png", "@@", 1000)
            .is_err());
        assert!(VisionTokenPolicy::Generic
            .fit_dimensions(
                1024,
                1024,
                VisionTokenPolicy::Generic.min_readable_tokens(1024, 1024) - 1
            )
            .is_err());
        assert_eq!(
            VisionTokenPolicy::Generic
                .fit_dimensions(1, 10000, estimate_image_tokens(1, 512))
                .unwrap(),
            (1, 512)
        );
    }

    #[test]
    fn shrinking_an_animation_does_not_silently_send_one_frame() {
        let mut bytes = Vec::new();
        let frame = image::Frame::new(image::RgbaImage::new(640, 64));
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frames([frame.clone(), frame])
            .unwrap();
        let data = STANDARD.encode(bytes);
        let budget = VisionTokenPolicy::Generic.min_readable_tokens(640, 64);
        let error = VisionTokenPolicy::Generic
            .resize("image/gif", &data, budget)
            .err()
            .expect("resizing an animated image must fail explicitly");
        assert!(error.to_string().contains("animated GIF"));
        let original = VisionTokenPolicy::Generic
            .resize("image/gif", &data, usize::MAX)
            .unwrap();
        assert_eq!(original.data_base64, data);
    }
}
