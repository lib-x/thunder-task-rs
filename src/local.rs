//! Local NAS client mode.
//!
//! The official Xunlei NAS client (the `pan-xunlei-com` package that runs on
//! Synology/UGREEN/LazyCat boxes) serves its own web UI on a local port and
//! injects a session token into that page:
//!
//! ```html
//! <script>function uiauth(value){ return "<token>" }</script>
//! ```
//!
//! The client hands this token to its own frontend, which then calls the local
//! API (`/device/*`, `/drive/v1/*`) with a `pan-auth` header. This module does
//! the same thing: fetch the page, take the token the client published for its
//! own UI, and talk to the same API.
//!
//! Nothing here is forged or reverse-engineered: the token is the one the
//! client itself serves to its frontend, and the server applies the same
//! account limits it applies to the official UI — including the free-tier
//! daily task quota (`CREATE_USER_DOWNLOAD_TASK_LIMIT_INTERNAL`), which this
//! module can read via [`LocalNas::quota`] but never bypasses.

use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::error::{Error, Result};
use crate::types::{CreateTaskRequest, Page, Phase, Task};

/// Path prefix of the NAS client's CGI endpoint.
pub const CGI_PREFIX: &str = "/webman/3rdparty/pan-xunlei-com/index.cgi";

/// A client for the local Xunlei NAS service.
#[derive(Debug, Clone)]
pub struct LocalNas {
    base: Url,
    token: String,
    http: reqwest::Client,
}

/// Free-tier daily task quota as reported by the client.
#[derive(Debug, Clone, Deserialize)]
pub struct TaskQuota {
    /// Tasks allowed per day; `-1` means unlimited.
    pub limit: i64,
    /// Tasks already created today.
    #[serde(default)]
    pub usage: i64,
}

impl TaskQuota {
    /// Remaining tasks today, or `None` when unlimited.
    pub fn remaining(&self) -> Option<i64> {
        if self.limit < 0 {
            None
        } else {
            Some((self.limit - self.usage).max(0))
        }
    }
}

impl LocalNas {
    /// Base URL of the running NAS client, e.g. `http://127.0.0.1:2345`.
    pub fn base(&self) -> &Url {
        &self.base
    }

    /// Build with a token you obtained yourself.
    pub fn with_token(base: Url, token: impl Into<String>) -> Self {
        Self {
            base,
            token: token.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Override the HTTP transport.
    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// Connect by fetching the client's own page and taking the token it
    /// injects for its frontend. Tokens are long-lived but not permanent;
    /// call this again to refresh.
    pub async fn connect(base: Url) -> Result<Self> {
        let http = reqwest::Client::new();
        let page = http
            .get(base.clone())
            .send()
            .await?
            .text()
            .await
            .map_err(|error| Error::UnexpectedResponse(error.to_string()))?;
        let token = extract_ui_token(&page).ok_or_else(|| {
            Error::UnexpectedResponse(
                "no uiauth token in the page served by the NAS client".to_string(),
            )
        })?;
        Ok(Self { base, token, http })
    }

    fn api_url(&self, path: &str) -> Result<Url> {
        let path = format!("{CGI_PREFIX}/{}", path.trim_start_matches('/'));
        self.base
            .join(&path)
            .map_err(|error| Error::UnexpectedResponse(error.to_string()))
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let url = self.api_url(path)?;
        let resp = self
            .http
            .get(url)
            .header("pan-auth", &self.token)
            .send()
            .await?;
        decode(resp).await
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let url = self.api_url(path)?;
        let resp = self
            .http
            .post(url)
            .header("pan-auth", &self.token)
            .json(&body)
            .send()
            .await?;
        decode(resp).await
    }

    /// Device information: platform, account, download paths and disk usage.
    pub async fn device_info(&self) -> Result<Value> {
        self.get("device/info").await
    }

    /// Free-tier daily task quota as the client sees it.
    pub async fn quota(&self) -> Result<TaskQuota> {
        let about = self
            .get("drive/v1/about?with_quotas=CREATE_USER_DOWNLOAD_TASK_LIMIT_INTERNAL")
            .await?;
        let quota = about
            .get("quotas")
            .and_then(|q| q.get("CREATE_USER_DOWNLOAD_TASK_LIMIT_INTERNAL"))
            .ok_or_else(|| {
                Error::UnexpectedResponse("quota not reported by the client".to_string())
            })?;
        serde_json::from_value(quota.clone())
            .map_err(|error| Error::UnexpectedResponse(error.to_string()))
    }

    /// List tasks. `limit` caps the page size when set.
    pub async fn tasks(&self, limit: Option<u32>) -> Result<Vec<Task>> {
        let path = match limit {
            Some(limit) => format!("drive/v1/tasks?limit={limit}"),
            None => "drive/v1/tasks".to_string(),
        };
        let value = self.get(&path).await?;
        let page: Page<Task> = serde_json::from_value(value)
            .map_err(|error| Error::UnexpectedResponse(error.to_string()))?;
        Ok(page.tasks)
    }

    /// Analyze a link before submitting it: file name, size, file count and
    /// the torrent infohash when applicable.
    pub async fn analyze(&self, url: &str) -> Result<Value> {
        self.post(
            "drive/v1/resource/list",
            serde_json::json!({ "urls": url, "space": "" }),
        )
        .await
    }

    /// Create a download task. Returns the server's response.
    ///
    /// On a free account this returns an API error with the client's
    /// "payment required" status once the daily quota is exhausted; read
    /// [`LocalNas::quota`] to report that to users instead of failing blindly.
    pub async fn create_task(&self, request: CreateTaskRequest) -> Result<Value> {
        let body = serde_json::to_value(request)
            .map_err(|error| Error::UnexpectedResponse(error.to_string()))?;
        self.post("drive/v1/task", body).await
    }

    /// Set a task's phase (pause, resume, delete).
    pub async fn operate_task(&self, id: &str, space: &str, phase: Phase) -> Result<()> {
        let body = serde_json::json!({
            "id": id,
            "space": space,
            "set_params": { "spec": format!("{{\"phase\":\"{}\"}}", phase.as_str()) },
        });
        self.patch(&format!("drive/v1/task/{id}"), body).await?;
        Ok(())
    }

    async fn patch(&self, path: &str, body: Value) -> Result<Value> {
        let url = self.api_url(path)?;
        let resp = self
            .http
            .patch(url)
            .header("pan-auth", &self.token)
            .json(&body)
            .send()
            .await?;
        decode(resp).await
    }

    /// Delete tasks by id.
    pub async fn delete_tasks(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut url = self.api_url("drive/v1/tasks")?;
        for id in ids {
            url.query_pairs_mut().append_pair("task_ids", id);
        }
        let resp = self
            .http
            .delete(url)
            .header("pan-auth", &self.token)
            .send()
            .await?;
        decode(resp).await?;
        Ok(())
    }
}

/// Pull the token the client injects for its own frontend.
pub fn extract_ui_token(page: &str) -> Option<String> {
    let marker = "function uiauth(value){ return \"";
    let start = page.find(marker)? + marker.len();
    let rest = &page[start..];
    let end = rest.find('"')?;
    let token = &rest[..end];
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

async fn decode(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let message = body
            .get("message")
            .or_else(|| body.get("error"))
            .or_else(|| body.get("error_description"))
            .and_then(Value::as_str)
            .unwrap_or("request failed")
            .to_string();
        return Err(Error::Api {
            code: Some(status.as_u16().to_string()),
            message,
        });
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_injected_token() {
        let page = r#"<script>
      function uiauth(value){ return "eyJhbGciOiJIUzI1NiJ9.abc.def" }
    </script>"#;
        assert_eq!(
            extract_ui_token(page).as_deref(),
            Some("eyJhbGciOiJIUzI1NiJ9.abc.def")
        );
    }

    #[test]
    fn reports_missing_token() {
        assert_eq!(extract_ui_token("<html>no token here</html>"), None);
        assert_eq!(
            extract_ui_token(r#"function uiauth(value){ return "" }"#),
            None
        );
    }

    #[test]
    fn quota_remaining_handles_unlimited_and_exhausted() {
        let unlimited = TaskQuota {
            limit: -1,
            usage: 9,
        };
        assert_eq!(unlimited.remaining(), None);
        let used = TaskQuota { limit: 3, usage: 3 };
        assert_eq!(used.remaining(), Some(0));
        let left = TaskQuota { limit: 3, usage: 1 };
        assert_eq!(left.remaining(), Some(2));
    }

    #[test]
    fn cgi_paths_are_built_under_the_client_prefix() {
        let nas = LocalNas::with_token(Url::parse("http://127.0.0.1:2345").unwrap(), "tok");
        assert_eq!(
            nas.api_url("device/info").unwrap().path(),
            "/webman/3rdparty/pan-xunlei-com/index.cgi/device/info"
        );
    }
}
