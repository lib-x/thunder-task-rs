//! Tolerant API models and download-task request construction.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// An authorized device, including unrecognized server fields.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Device {
    /// Device identifier.
    #[serde(alias = "device_id")]
    pub id: String,
    /// Human-readable device name.
    #[serde(alias = "device_name")]
    pub name: String,
    /// Space used to route tasks to this device.
    #[serde(alias = "device_space")]
    pub space: String,
    /// Operating platform, when reported.
    pub platform: Option<String>,
    /// Device category, when reported.
    pub device_type: Option<String>,
    /// Server-defined device status.
    pub status: Option<Value>,
    /// Additional server fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A task lifecycle phase; unknown server values are preserved verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Phase {
    /// Waiting to run.
    #[default]
    Pending,
    /// Currently running.
    Running,
    /// Paused by the user.
    Paused,
    /// Failed.
    Error,
    /// Finished successfully.
    Complete,
    /// An unrecognized phase, including operation-only values.
    Unknown(String),
}

impl Phase {
    /// Return the exact API phase string.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "PHASE_TYPE_PENDING",
            Self::Running => "PHASE_TYPE_RUNNING",
            Self::Paused => "PHASE_TYPE_PAUSED",
            Self::Error => "PHASE_TYPE_ERROR",
            Self::Complete => "PHASE_TYPE_COMPLETE",
            Self::Unknown(value) => value,
        }
    }
}

impl Serialize for Phase {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Phase {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "PHASE_TYPE_PENDING" => Self::Pending,
            "PHASE_TYPE_RUNNING" => Self::Running,
            "PHASE_TYPE_PAUSED" => Self::Paused,
            "PHASE_TYPE_ERROR" => Self::Error,
            "PHASE_TYPE_COMPLETE" => Self::Complete,
            _ => Self::Unknown(value),
        })
    }
}

/// A download task with tolerant numeric fields and extension data.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Task {
    /// Task identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Current lifecycle phase.
    pub phase: Phase,
    /// Target device/account space.
    pub space: Option<String>,
    /// Server task type.
    #[serde(rename = "type")]
    pub task_type: Option<String>,
    /// File size (the API may use a string or a number).
    pub file_size: Option<Value>,
    /// Progress in the server's representation.
    pub progress: Option<Value>,
    /// Selected sub-file index in the server's representation.
    pub sub_file_index: Option<Value>,
    /// Task-specific parameters.
    pub params: Value,
    /// Additional server fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The payload of a task submission: either a link or an uploaded file.
#[derive(Debug, Clone)]
pub enum TaskSource {
    /// Magnet, HTTP(S), FTP, ed2k or thunder link.
    Url(String),
    /// A file (e.g. a `.torrent`) already uploaded to the drive, referenced
    /// by id. Optionally restrict to selected sub-file indexes.
    File {
        /// Drive file id of the uploaded torrent / resource.
        file_id: String,
        /// Selected sub-file indexes (0-based), empty means all.
        sub_file_index: Vec<u32>,
    },
}

/// A download request, serialized into the API's nested task body.
#[derive(Debug, Clone)]
pub struct CreateTaskRequest {
    /// Where the bytes come from.
    pub source: TaskSource,
    /// Optional display name.
    pub name: Option<String>,
    /// Optional output filename.
    pub file_name: Option<String>,
    /// Optional destination folder identifier.
    pub parent_folder_id: Option<String>,
    /// Target device space, or the empty personal-drive space.
    pub space: String,
    /// Number of files in the submitted task (defaults to one).
    pub total_file_count: u64,
    /// Optional MIME type hint for the source (useful for file tasks).
    pub mime_type: Option<String>,
}

impl CreateTaskRequest {
    /// Construct a download request from a link, for the personal-drive space.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            source: TaskSource::Url(url.into()),
            name: None,
            file_name: None,
            parent_folder_id: None,
            space: crate::SPACE_USER.into(),
            total_file_count: 1,
            mime_type: None,
        }
    }

    /// Construct a download request from an uploaded file (e.g. a torrent).
    pub fn from_file(file_id: impl Into<String>) -> Self {
        Self {
            source: TaskSource::File {
                file_id: file_id.into(),
                sub_file_index: Vec::new(),
            },
            name: None,
            file_name: None,
            parent_folder_id: None,
            space: crate::SPACE_USER.into(),
            total_file_count: 1,
            mime_type: None,
        }
    }

    /// Restrict a file-sourced task to specific sub-file indexes.
    pub fn with_sub_file_index(mut self, indexes: Vec<u32>) -> Self {
        if let TaskSource::File { sub_file_index, .. } = &mut self.source {
            *sub_file_index = indexes;
        }
        self
    }

    /// Set a MIME type hint.
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    /// Set the display name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the output filename.
    pub fn with_file_name(mut self, file_name: impl Into<String>) -> Self {
        self.file_name = Some(file_name.into());
        self
    }

    /// Set the destination folder.
    pub fn with_parent_folder_id(mut self, parent_folder_id: impl Into<String>) -> Self {
        self.parent_folder_id = Some(parent_folder_id.into());
        self
    }

    /// Route the task to a device's space.
    pub fn with_space(mut self, space: impl Into<String>) -> Self {
        self.space = space.into();
        self
    }

    /// Set the number of files.
    pub fn with_total_file_count(mut self, total_file_count: u64) -> Self {
        self.total_file_count = total_file_count;
        self
    }
}

impl Serialize for CreateTaskRequest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (task_type, mut params) = match &self.source {
            TaskSource::Url(url) => (
                crate::TYPE_USER_DOWNLOAD,
                serde_json::json!({
                    "url": url,
                    "total_file_count": self.total_file_count.to_string(),
                }),
            ),
            TaskSource::File {
                file_id,
                sub_file_index,
            } => {
                let mut p = serde_json::json!({
                    "file_id": file_id,
                    "total_file_count": self.total_file_count.to_string(),
                });
                if !sub_file_index.is_empty() {
                    p["sub_file_index"] = sub_file_index
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                        .into();
                }
                (crate::TYPE_USER_DOWNLOAD, p)
            }
        };
        if let Some(folder) = &self.parent_folder_id {
            params["parent_folder_id"] = folder.clone().into();
        }
        if let Some(mime) = &self.mime_type {
            params["mime_type"] = mime.clone().into();
        }
        let mut body = serde_json::json!({
            "type": task_type,
            "file_size": "0",
            "space": self.space,
            "params": params,
        });
        if let Some(name) = &self.name {
            body["name"] = name.clone().into();
        }
        if let Some(file_name) = &self.file_name {
            body["file_name"] = file_name.clone().into();
        }
        body.serialize(serializer)
    }
}

/// One list response page, accepting `tasks`, `list`, or a bare array.
#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    /// Items in this page.
    pub tasks: Vec<T>,
    /// Token for the next page, if supplied.
    pub next_page_token: Option<String>,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Page<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged, bound(deserialize = "T: Deserialize<'de>"))]
        enum WirePage<T> {
            Array(Vec<T>),
            Object {
                #[serde(alias = "list")]
                tasks: Vec<T>,
                #[serde(default)]
                next_page_token: Option<String>,
            },
        }
        Ok(match WirePage::deserialize(deserializer)? {
            WirePage::Array(tasks) => Self {
                tasks,
                next_page_token: None,
            },
            WirePage::Object {
                tasks,
                next_page_token,
            } => Self {
                tasks,
                next_page_token,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn phase_round_trips() {
        for phase in [
            Phase::Pending,
            Phase::Running,
            Phase::Paused,
            Phase::Error,
            Phase::Complete,
            Phase::Unknown("PHASE_TYPE_DELETE".into()),
        ] {
            let encoded = serde_json::to_value(&phase).unwrap();
            assert_eq!(encoded, json!(phase.as_str()));
            assert_eq!(serde_json::from_value::<Phase>(encoded).unwrap(), phase);
        }
    }

    #[test]
    fn create_body_defaults_and_options() {
        let request = CreateTaskRequest::new("magnet:?xt=example");
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "type": "user#download", "file_size": "0", "space": "",
                "params": {"url": "magnet:?xt=example", "total_file_count": "1"}
            })
        );
        let request = request
            .with_name("download")
            .with_file_name("file")
            .with_parent_folder_id("folder")
            .with_space("nas")
            .with_total_file_count(3);
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "type": "user#download", "file_size": "0", "space": "nas",
                "name": "download", "file_name": "file",
                "params": {"url": "magnet:?xt=example", "total_file_count": "3", "parent_folder_id": "folder"}
            })
        );
    }

    #[test]
    fn pages_accept_all_shapes() {
        for body in [
            json!([]),
            json!({"tasks": []}),
            json!({"list": [], "extra": true}),
        ] {
            assert!(serde_json::from_value::<Page<Task>>(body)
                .unwrap()
                .tasks
                .is_empty());
        }
        let page: Page<Task> =
            serde_json::from_value(json!({"tasks": [{"id": "task"}], "next_page_token": "next"}))
                .unwrap();
        assert_eq!(page.tasks[0].phase, Phase::Pending);
        assert_eq!(page.next_page_token.as_deref(), Some("next"));
    }

    #[test]
    fn models_preserve_extensions_and_numeric_variants() {
        let device: Device = serde_json::from_value(
            json!({"device_id": "nas", "device_space": "space", "online": true}),
        )
        .unwrap();
        assert_eq!(device.id, "nas");
        assert_eq!(device.space, "space");
        assert_eq!(device.extra["online"], true);
        for size in [json!(10), json!("10")] {
            let task: Task = serde_json::from_value(json!({"file_size": size, "new": 1})).unwrap();
            assert_eq!(task.file_size, Some(size));
            assert_eq!(task.extra["new"], 1);
        }
    }
}
