//! Named tasks (ecosystem phase 1c): the phone's half. It asks the
//! computer for its task names, starts one, and keeps each run's latest
//! result as the computer sends it on the control link (`running`, then
//! `done` or `refused`), for the app to look at.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::NetworkPacket;

pub(crate) const TASKS: &str = "kdeconnect.pocketdesk.tasks";
pub(crate) const TASK_RUN: &str = "kdeconnect.pocketdesk.task_run";
pub(crate) const TASK_RESULT: &str = "kdeconnect.pocketdesk.task_result";

/// Runs whose results are kept, at most: the newest.
const MAX_KEPT: usize = 16;

/// The latest result of each run, by the run's id.
static RESULTS: LazyLock<Mutex<VecDeque<(String, serde_json::Value)>>> =
    LazyLock::new(Default::default);

/// A result the computer sent: kept under its run's id, replacing the last.
pub(crate) fn receive(body: &serde_json::Value) {
    let Some(id) = body.get("id").and_then(|v| v.as_str()) else {
        return;
    };
    let mut results = RESULTS.lock().unwrap_or_else(|e| e.into_inner());
    results.retain(|(kept, _)| kept != id);
    results.push_back((id.to_string(), body.clone()));
    while results.len() > MAX_KEPT {
        results.pop_front();
    }
}

/// The latest result of run `id`, as JSON, or empty while none has come.
pub(crate) fn result_json(id: &str) -> String {
    RESULTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(kept, _)| kept == id)
        .map(|(_, body)| body.to_string())
        .unwrap_or_default()
}

/// The computer's tasks: `{"tasks": [{"name", "ask"}]}`.
pub async fn list_tasks(address: String, port: u16) -> Result<String, String> {
    let request = NetworkPacket::new(TASKS).with_body(serde_json::json!({}));
    crate::api::query_peer(&address, port, &request, TASKS, Duration::from_secs(5))
        .await
        .map(|body| body.to_string())
}

/// Start task `name` as run `id`; its results come to [`result_json`]. Only
/// on the live control link, whose reader takes the results: a one-off
/// connection would start the task and lose every one of them.
pub async fn run_task(address: String, port: u16, name: String, id: String) -> Result<(), String> {
    let packet =
        NetworkPacket::new(TASK_RUN).with_body(serde_json::json!({ "name": name, "id": id }));
    crate::api::send_on_live(&address, port, &packet).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_latest_result_of_a_run_is_kept_and_old_runs_go() {
        receive(&json!({"id": "t-run-1", "state": "running"}));
        receive(&json!({"id": "t-run-1", "state": "done", "code": 0}));
        assert!(result_json("t-run-1").contains("\"done\""));
        assert_eq!(result_json("t-no-such-run"), "");
        for i in 0..MAX_KEPT {
            receive(&json!({"id": format!("t-fill-{i}"), "state": "running"}));
        }
        assert_eq!(result_json("t-run-1"), "", "the oldest made room");
        receive(&json!({"state": "done"}));
    }
}
