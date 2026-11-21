use log::{error, info};
use openbitfun_core::agentic::insights::{InsightsReport, InsightsReportMeta, InsightsService};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateInsightsRequest {
    pub days: Option<u32>,
    pub model_id: Option<String>,
    pub generation_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadInsightsReportRequest {
    pub path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelInsightsRequest {
    pub generation_id: Option<String>,
}

#[tauri::command]
pub async fn generate_insights(request: GenerateInsightsRequest) -> Result<InsightsReport, String> {
    let days = request.days.unwrap_or(30);
    let model_id = request.model_id;
    info!(
        "Generating insights for the last {} days with model selector {:?}",
        days, model_id
    );

    InsightsService::generate(days, model_id, request.generation_id)
        .await
        .map_err(|e| {
            error!("Failed to generate insights: {}", e);
            format!("Failed to generate insights: {}", e)
        })
}

#[tauri::command]
pub async fn get_latest_insights() -> Result<Vec<InsightsReportMeta>, String> {
    InsightsService::load_latest_reports().await.map_err(|e| {
        error!("Failed to load latest insights: {}", e);
        format!("Failed to load latest insights: {}", e)
    })
}

#[tauri::command]
pub async fn load_insights_report(
    request: LoadInsightsReportRequest,
) -> Result<InsightsReport, String> {
    InsightsService::load_report(&request.path)
        .await
        .map_err(|e| {
            error!("Failed to load insights report: {}", e);
            format!("Failed to load insights report: {}", e)
        })
}

#[tauri::command]
pub async fn has_insights_data(request: GenerateInsightsRequest) -> Result<bool, String> {
    let days = request.days.unwrap_or(30);
    InsightsService::has_data(days).await.map_err(|e| {
        error!("Failed to check insights data: {}", e);
        format!("Failed to check insights data: {}", e)
    })
}

#[tauri::command]
pub async fn cancel_insights_generation(
    request: Option<CancelInsightsRequest>,
) -> Result<(), String> {
    let generation_id = request.and_then(|request| request.generation_id);
    InsightsService::cancel(generation_id.as_deref())
        .await
        .map_err(|e| {
            error!("Failed to cancel insights generation: {}", e);
            e
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_legacy_generation_requests_and_correlated_requests() {
        let legacy: GenerateInsightsRequest =
            serde_json::from_value(serde_json::json!({ "days": 7, "modelId": "primary" })).unwrap();
        assert_eq!(legacy.days, Some(7));
        assert_eq!(legacy.model_id.as_deref(), Some("primary"));
        assert_eq!(legacy.generation_id, None);

        let current: GenerateInsightsRequest =
            serde_json::from_value(serde_json::json!({ "generationId": "run-new" })).unwrap();
        let cancel: CancelInsightsRequest =
            serde_json::from_value(serde_json::json!({ "generationId": "run-new" })).unwrap();
        assert_eq!(current.generation_id, cancel.generation_id);
    }
}
