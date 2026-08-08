use super::*;
use crate::application::apply as apply_service;

pub(super) use apply_service::{apply_ready_automatically, resolve_artwork};

pub async fn start_apply(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    if !s
        .try_claim_workflow(WorkflowPhase::Apply, "Writing corrected copies", false)
        .await
    {
        return Err(ApiError::conflict("workflow is already running"));
    }
    let prepared = match apply_service::prepare_apply(&s).await {
        Ok(prepared) => prepared,
        Err(error) => {
            s.reset_workflow(WorkflowPhase::Idle, "Ready").await;
            return Err(error.into());
        }
    };
    if prepared.selected_count == 0 {
        s.reset_workflow(WorkflowPhase::Idle, "Ready").await;
        return Err(ApiError::validation(
            "No identified tracks are ready to write",
        ));
    }
    let apply_service::PreparedApply {
        items,
        selected_count,
        outputs,
        duplicates_skipped,
    } = prepared;
    let state = s.clone();
    tokio::spawn(async move {
        apply_service::finish_apply_workflow(state, items).await;
    });
    Ok(Json(serde_json::json!({
        "started": true,
        "count": selected_count,
        "outputs": outputs,
        "duplicates_skipped": duplicates_skipped
    })))
}
