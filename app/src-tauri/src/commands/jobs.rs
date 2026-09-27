use tauri::State;

use crate::dto::JobDto;
use crate::error::AppResult;
use crate::state::AppState;

#[tauri::command]
pub async fn cancel_job(state: State<'_, AppState>, id: u64) -> AppResult<bool> {
    Ok(state.jobs.cancel(id))
}

#[tauri::command]
pub async fn list_jobs(state: State<'_, AppState>) -> AppResult<Vec<JobDto>> {
    Ok(state.jobs.list().iter().map(JobDto::from).collect())
}
