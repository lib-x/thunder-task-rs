# thunder-task-rs — implementation spec for codex

## Goal
Rust library implementing a client for the Xunlei (迅雷) Pan cloud API used by
official mobile/NAS clients. Repo: github.com/lib-x/thunder-task-rs.
Local working dir: /home/czyt/Projects/thunder-task-rs

Already present: Cargo.toml, src/lib.rs (module list + consts), src/error.rs
(Error enum + Result alias). Complete the remaining modules to match lib.rs.

## Known API facts (evidence from reverse-engineering official clients)

- Auth host: `https://xluser-ssl.xunlei.com`
- OAuth endpoints (standard RFC 8628 device flow, seen in APK strings):
  - `POST /v1/auth/device/code` (client_id, scope) → device_code, user_code,
    verification_url, verification_url_qrcode, expires_in, interval
  - `POST /v1/auth/token` (client_id, client_secret, grant_type=
    `urn:ietf:params:oauth:grant-type:device_code`, device_code)
    → access_token, refresh_token, token_type, expires_in
    errors: `authorization_pending`, `slow_down`, `expired_token`, `access_denied`
  - refresh: same endpoint, grant_type=refresh_token
- Drive host: `https://api-gateway-pan.xunlei.com`, path prefix `/xlppc/drive/v1`
  (mobile) — NAS local client uses `/drive/v1` on its own host. Library should
  use the gateway prefix.
- Task endpoints:
  - `POST {prefix}/task` — create task
  - `GET {prefix}/tasks` — list (filters param is a URL-encoded JSON string)
  - `GET {prefix}/tasks/{id}` — single task
  - `PATCH {prefix}/task/{id}` — operate: body has set_params.spec = JSON string
    `{"phase":"PHASE_TYPE_RUNNING"|"PHASE_TYPE_PAUSED"|"PHASE_TYPE_DELETE"...}`
    Some clients send PATCH via `POST /xlppc/method/patch/drive/v1/task/{id}`;
    implement PATCH first, fall back not needed for now.
  - `DELETE {prefix}/tasks?task_ids=a&task_ids=b`
- Create-task body (from NAS web UI code):
  ```json
  {
    "type": "user#download",
    "name": "<optional display name>",
    "file_name": "<optional>",
    "file_size": "0",
    "space": "<space, often empty string>",
    "params": {
      "url": "<magnet|http|thunder link>",
      "parent_folder_id": "<optional target dir id>",
      "total_file_count": "1"
    }
  }
  ```
  Task type constants observed: `user#download` (URL tasks), `user#app`,
  `user#nfo`, `user#runner`. For submitting to a specific NAS device the
  mobile client passes `space` = device-space value obtained from the device
  record (fields seen in clients: `space`, `device_space`, `space=device_id`
  routing, task param `target` = same space). Devices listing: mobile client
  has `/v1/user/authorized/devices` on the auth host and drive-side device
  listing; implement `GET {DRIVE_HOST}/xlppc/drive/v1/devices` and
  `GET {AUTH_HOST}/v1/user/authorized/devices` both, return whichever works
  with a unified Device struct.
- Auth header on drive APIs: `Authorization: Bearer <access_token>`.
- Response envelope: either raw object or `{"error": "...", "error_code": ...,
  "error_description": "..."}` on failure; success task creation returns the
  task object. Be tolerant: check `error` field even on 200.

## Module requirements

### src/types.rs
- `Device { id, name, space, platform?, device_type?, status?, extra map }`
  tolerant with #[serde(default)] and flatten
- `Task { id, name, phase, space?, task_type? (serde rename "type"), file_size?,
  progress?, sub_file_index?, params (json value), extra map }`
- `Phase` enum: PHASE_TYPE_PENDING / RUNNING / PAUSED / ERROR / COMPLETE,
  serde string, Unknown(String) fallback
- `CreateTaskRequest` builder-ish struct with `url`, optional name/file_name,
  `parent_folder_id`, `space` (target device space), `total_file_count`
- `Page<T>` for list responses: `{ tasks: [...], next_page_token? }` tolerant
  (also accept `list` or bare array)

### src/client.rs
- `ClientConfig { credentials: ClientCredentials, http: reqwest::Client,
  drive_host: Url, auth_host: Url, tokens: Option<TokenSet> }` with
  `with_http`, `with_hosts` builders
- `Client::new(cred)`, `Client::from_session(Session)` (restores tokens)
- `login_interactive()` — full device flow: request device code, print user
  code + verification URL, render QR to terminal (use `qrcode` crate,
  render::<char> with half-block chars), poll with interval (respect
  `slow_down` by +5s), return Session. Caller prints; keep IO minimal (write
  to stderr).
- `session()` -> Option<&Session>
- internal `authed_request(method, path, body)` that:
  - ensures a token exists (else Error::NotAuthenticated)
  - adds Authorization Bearer
  - on 401, tries refresh_token once, then retries
- expose `raw_request` for extension

### src/device.rs
- `Client::devices()` -> Vec<Device>: try gateway devices endpoint first,
  fall back to auth-host authorized-devices; map shapes tolerantly

### src/task.rs
- `Client::create_task(CreateTaskRequest) -> Task`
- `Client::tasks(filter: TaskFilter) -> Vec<Task>` where TaskFilter has
  phases (Vec<Phase>), page_token, limit; filters serialized as JSON string
  into `filters` query param like `{"phase":{"in":"PHASE_TYPE_RUNNING,..."}}`
- `Client::task(id) -> Task`
- `Client::operate_task(id, space, phase: Phase) -> ()` (PATCH set_params.spec)
- `Client::delete_tasks(ids: &[String]) -> ()`

### Quality bar
- rustfmt clean, `cargo clippy -- -D warnings` clean
- unit tests for: filter serialization, phase serde, create-body serialization
  (no network in tests)
- async everywhere (tokio), reqwest with rustls
- no unwrap in library code paths (expect allowed for static URL parsing)
- errors via crate::error::{Error, Result}
- docs: doc-comments on all public items; crate-level doc exists already
- Ed25519 of repo conventions: MIT license header not required

### Do NOT
- do not invent client_id/client_secret values; ClientCredentials must be
  provided by the caller (documented)
- do not add bypass logic for quotas or signature forgery; plain OAuth +
  Bearer only
- do not use blocking IO

## Deliverable
Complete src/types.rs, src/client.rs, src/device.rs, src/task.rs so that
`cargo build` and `cargo test` pass with the existing Cargo.toml. Keep lib.rs
module list as is (you may add `pub use` items there if helpful).
