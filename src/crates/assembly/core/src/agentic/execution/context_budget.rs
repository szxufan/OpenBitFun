//! Input admission budgets shared by ordinary requests and pool routing.

use crate::service::config::types::automatic_max_output_tokens;
use openbitfun_core_types::model_pool::ModelPoolContextBudget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompressionTriggerBudget {
    pub context_window: usize,
    pub input_limit: usize,
    pub output_reserve_tokens: usize,
    pub safety_reserve_tokens: usize,
}

impl CompressionTriggerBudget {
    const SAFETY_RESERVE_TOKENS: usize = 10_000;

    /// The active model or pool owns the request budget. Persisted session
    /// windows can describe a previous model or a legacy creation default.
    pub(super) fn for_request(
        model: &openbitfun_core_types::AIConfig,
        pool: Option<ModelPoolContextBudget>,
    ) -> Self {
        match pool {
            Some(budget) => Self::for_pool(budget),
            None => Self::for_model(model.context_window as usize, model.max_tokens),
        }
    }

    pub(super) fn for_model(context_window: usize, configured_max_tokens: Option<u32>) -> Self {
        let output_reserve_tokens = configured_max_tokens
            .map(|value| value as usize)
            .unwrap_or_else(|| automatic_max_output_tokens(context_window as u32) as usize);
        let safety_reserve_tokens = Self::SAFETY_RESERVE_TOKENS;
        Self {
            context_window,
            input_limit: context_window
                .saturating_sub(output_reserve_tokens + safety_reserve_tokens),
            output_reserve_tokens,
            safety_reserve_tokens,
        }
    }

    pub(super) fn for_pool(budget: ModelPoolContextBudget) -> Self {
        let context_window = budget.context_window as usize;
        let input_limit = budget.input_limit as usize;
        let safety_reserve_tokens = Self::SAFETY_RESERVE_TOKENS.min(context_window);
        Self {
            context_window,
            input_limit,
            // This is an equivalent reserve for the common window, not a new
            // max_tokens setting for any member's provider request.
            output_reserve_tokens: context_window
                .saturating_sub(input_limit + safety_reserve_tokens),
            safety_reserve_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::config::types::AIModelConfig;

    #[test]
    fn compression_budget_follows_each_selected_model() {
        for (window, max_output, input_limit) in
            [(1_048_576, 384_000, 654_576), (64_000, 16_000, 38_000)]
        {
            let model = openbitfun_core_types::AIConfig::try_from(AIModelConfig {
                context_window: Some(window),
                max_tokens: Some(max_output),
                ..Default::default()
            })
            .unwrap();
            let budget = CompressionTriggerBudget::for_request(&model, None);
            assert_eq!(budget.context_window, window as usize);
            assert_eq!(budget.input_limit, input_limit);
            assert_eq!(47_544 >= budget.input_limit, window == 64_000);
        }
    }
}
