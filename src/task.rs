//! Download-task listing, creation, and lifecycle operations.

use reqwest::Method;
use serde::{Serialize, Serializer};
use serde_json::{json, Value};
use url::Url;

use crate::auth::decode;
use crate::types::{CreateTaskRequest, Page, Phase, Task};
use crate::{Client, Error, Result, DRIVE_PREFIX};

/// Optional phase and pagination controls for a task list request.
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    /// Phases to include; empty means no phase filter.
    pub phases: Vec<Phase>,
    /// Page token obtained from a previous API response.
    pub page_token: Option<String>,
    /// Maximum number of tasks in this page.
    pub limit: Option<u32>,
}

impl Serialize for TaskFilter {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let filters = if self.phases.is_empty() {
            json!({})
        } else {
            json!({"phase": {"in": self.phases.iter().map(Phase::as_str).collect::<Vec<_>>().join(",")}})
        };
        filters.serialize(serializer)
    }
}

impl Client {
    /// Analyze a link before submission: returns the resource tree (for
    /// multi-file torrents) and metadata. Maps to the resource/list API the
    /// official clients call before showing the file-picker dialog.
    pub async fn analyze_link(&mut self, url: &str, space: &str) -> Result<serde_json::Value> {
        let body = serde_json::json!({ "urls": url, "space": space });
        decode(
            self.authed_request(
                Method::POST,
                &format!("{DRIVE_PREFIX}/resource/list"),
                Some(body),
            )
            .await?,
        )
    }

    /// Create a URL download, optionally routed to a device space.
    pub async fn create_task(&mut self, request: CreateTaskRequest) -> Result<Task> {
        let body = serde_json::to_value(request)
            .map_err(|error| Error::UnexpectedResponse(error.to_string()))?;
        decode(
            self.authed_request(Method::POST, &format!("{DRIVE_PREFIX}/task"), Some(body))
                .await?,
        )
    }

    /// Fetch one page of tasks matching the supplied filters.
    ///
    /// Use [`Client::raw_request`] when the next-page token itself is needed.
    pub async fn tasks(&mut self, filter: TaskFilter) -> Result<Vec<Task>> {
        let mut url = self.drive_url(&format!("{DRIVE_PREFIX}/tasks"))?;
        append_filter(&mut url, &filter)?;
        let page: Page<Task> = decode(self.request_url(Method::GET, url, None).await?)?;
        Ok(page.tasks)
    }

    /// Fetch a task by its identifier.
    pub async fn task(&mut self, id: &str) -> Result<Task> {
        let url = task_url(self.drive_url(&format!("{DRIVE_PREFIX}/tasks/"))?, id)?;
        decode(self.request_url(Method::GET, url, None).await?)
    }

    /// Set a task phase in the selected space using the PATCH API.
    pub async fn operate_task(&mut self, id: &str, space: &str, phase: Phase) -> Result<()> {
        let url = task_url(self.drive_url(&format!("{DRIVE_PREFIX}/task/"))?, id)?;
        self.request_url(Method::PATCH, url, Some(operation_body(space, phase)))
            .await?;
        Ok(())
    }

    /// Pause a task. Looks up its space first when not supplied.
    pub async fn pause_task(&mut self, id: &str) -> Result<()> {
        let space = self.task_space(id).await?;
        self.operate_task(id, &space, Phase::Paused).await
    }

    /// Resume a task. Looks up its space first when not supplied.
    pub async fn resume_task(&mut self, id: &str) -> Result<()> {
        let space = self.task_space(id).await?;
        self.operate_task(id, &space, Phase::Running).await
    }

    async fn task_space(&mut self, id: &str) -> Result<String> {
        Ok(self.task(id).await?.space.unwrap_or_default())
    }

    /// Delete tasks using a repeated `task_ids` query parameter.
    /// An empty slice is a no-op to avoid an unfiltered deletion request.
    pub async fn delete_tasks(&mut self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut url = self.drive_url(&format!("{DRIVE_PREFIX}/tasks"))?;
        for id in ids {
            url.query_pairs_mut().append_pair("task_ids", id);
        }
        self.request_url(Method::DELETE, url, None).await?;
        Ok(())
    }
}

fn append_filter(url: &mut Url, filter: &TaskFilter) -> Result<()> {
    let filters = serde_json::to_string(filter)
        .map_err(|error| Error::UnexpectedResponse(error.to_string()))?;
    let mut query = url.query_pairs_mut();
    query.append_pair("filters", &filters);
    if let Some(token) = &filter.page_token {
        query.append_pair("page_token", token);
    }
    if let Some(limit) = filter.limit {
        query.append_pair("limit", &limit.to_string());
    }
    Ok(())
}

fn task_url(mut url: Url, id: &str) -> Result<Url> {
    if id.is_empty() || id == "." || id == ".." {
        return Err(Error::UnexpectedResponse("invalid task identifier".into()));
    }
    url.path_segments_mut()
        .map_err(|_| Error::UnexpectedResponse("drive URL cannot hold path segments".into()))?
        .pop_if_empty()
        .push(id);
    Ok(url)
}

fn operation_body(space: &str, phase: Phase) -> Value {
    json!({"space": space, "set_params": {"spec": json!({"phase": phase}).to_string()}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_serialization_and_query_encoding() {
        assert_eq!(
            serde_json::to_value(TaskFilter::default()).unwrap(),
            json!({})
        );
        let filter = TaskFilter {
            phases: vec![Phase::Running, Phase::Paused],
            page_token: Some("a+b&c".into()),
            limit: Some(20),
        };
        let expected = json!({"phase": {"in": "PHASE_TYPE_RUNNING,PHASE_TYPE_PAUSED"}});
        assert_eq!(serde_json::to_value(&filter).unwrap(), expected);
        let mut url = Url::parse("https://example.org/tasks").unwrap();
        append_filter(&mut url, &filter).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            serde_json::from_str::<Value>(&pairs["filters"]).unwrap(),
            expected
        );
        assert_eq!(pairs["page_token"], "a+b&c");
        assert_eq!(pairs["limit"], "20");
    }

    #[test]
    fn operation_spec_is_a_json_string() {
        let body = operation_body("nas", Phase::Paused);
        assert_eq!(body["space"], "nas");
        let spec: Value =
            serde_json::from_str(body["set_params"]["spec"].as_str().unwrap()).unwrap();
        assert_eq!(spec, json!({"phase": "PHASE_TYPE_PAUSED"}));
    }

    #[test]
    fn identifiers_are_single_encoded_segments() {
        let url = task_url(Url::parse("https://example.org/tasks/").unwrap(), "a/b?c#d").unwrap();
        assert_eq!(url.path(), "/tasks/a%2Fb%3Fc%23d");
        assert!(url.query().is_none());
        assert!(url.fragment().is_none());
    }
}
