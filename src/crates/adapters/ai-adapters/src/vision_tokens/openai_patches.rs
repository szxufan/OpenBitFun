//! OpenAI's patch-based model sizing table and 32-pixel patch calculation.
//! https://developers.openai.com/api/docs/guides/images-vision#model-sizing-behavior

use super::{grid, scale_long_edge};

#[derive(Clone, Copy)]
enum Family {
    Astra6,
    Gpt56,
    Gpt55,
    Gpt54,
    Gpt52,
    Gpt41Mini,
}

#[derive(Clone, Copy)]
enum Detail {
    Low,
    High,
    Original,
}

#[derive(Clone, Copy)]
pub(crate) struct OpenAiPatches {
    family: Family,
    detail: Detail,
}

impl OpenAiPatches {
    pub(super) fn for_model(model: &str) -> Option<Self> {
        use Family::*;
        let family = match model {
            "gpt-6-astra" => Astra6,
            "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna" => Gpt56,
            "gpt-5.5" => Gpt55,
            "gpt-5.4" | "gpt-5.4-mini" | "gpt-5.4-nano" => Gpt54,
            "gpt-5.2" => Gpt52,
            "gpt-4.1-mini" => Gpt41Mini,
            _ => return None,
        };
        let detail = match family {
            Astra6 | Gpt56 | Gpt55 => Detail::Original,
            Gpt54 | Gpt52 | Gpt41Mini => Detail::High,
        };
        Some(Self { family, detail })
    }

    pub(super) fn with_detail(mut self, detail: Option<&str>) -> Self {
        self.detail = match detail {
            Some("low") => Detail::Low,
            Some("high") => Detail::High,
            Some("original") => Detail::Original,
            _ => return self,
        };
        self
    }

    pub(super) fn estimate(self, width: u32, height: u32) -> u64 {
        use Detail::*;
        use Family::*;
        let (max_edge, patch_budget) = match (self.family, self.detail) {
            (Gpt52 | Gpt41Mini, _) | (Gpt54, Low) => (2048, 6144),
            (_, Low) => (512, 256),
            (Astra6, High) => (65_535, 2500),
            (_, High) => (2048, 2500),
            (Gpt54 | Gpt55, Original) => (6000, 10_000),
            // This detail has no automatic patch-budget resize. In particular,
            // do not cap the estimate at the provider's separate rejection limit.
            (Astra6 | Gpt56, Original) => (65_535, u64::MAX),
        };
        let patches = resized_patches(width, height, max_edge, patch_budget);
        let percent = if matches!(self.family, Gpt41Mini) {
            162
        } else {
            120
        };
        (patches * percent).div_ceil(100)
    }
}

fn resized_patches(width: u32, height: u32, max_edge: u32, budget: u64) -> u64 {
    let (width, height) = if width.max(height) > max_edge {
        scale_long_edge(width, height, max_edge)
    } else {
        (width, height)
    };
    let patches = grid(width, height, 32);
    if patches <= budget {
        return patches;
    }

    // Scale to the target area, then snap the tighter axis to a whole patch.
    let width = f64::from(width);
    let height = f64::from(height);
    let scale = ((budget as f64 * 32.0 * 32.0) / (width * height)).sqrt();
    let scaled_w = width * scale / 32.0;
    let scaled_h = height * scale / 32.0;
    if scaled_w < 1.0 || scaled_h < 1.0 {
        // A panorama still needs one patch along its short axis.
        return budget;
    }
    let scale = (scaled_w.floor() * 32.0 / width).min(scaled_h.floor() * 32.0 / height);
    grid((width * scale) as u32, (height * scale) as u32, 32).min(budget)
}
