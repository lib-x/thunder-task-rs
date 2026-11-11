# thunder-task-rs

<img src="logo.png" alt="thunder-task-rs logo" width="240">

A Rust client for the Xunlei (迅雷) Pan cloud API, the same API used by the official mobile and NAS clients. It provides OAuth device-flow login, authorized device discovery, download task creation and management, and link analysis for resource metadata and torrent file selection.

Repository: [github.com/lib-x/thunder-task-rs](https://github.com/lib-x/thunder-task-rs)

For a command-line interface built on this library, see [thunder-remoted](https://github.com/ca-x/thunder-remoted).

## Installation

Add the library from Git and enable a Tokio runtime in your application:

```toml
[dependencies]
thunder-task-rs = { git = "https://github.com/lib-x/thunder-task-rs" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
serde_json = "1"
```

The crate is imported as `thunder_task_rs`. HTTP requests are asynchronous and use `reqwest` with Rustls TLS.

## Login

Authentication follows the OAuth 2.0 device authorization grant described by RFC 8628:

1. Request a device code with `POST https://xluser-ssl.xunlei.com/v1/auth/device/code`. The response includes `device_code`, `user_code`, `verification_url`, and polling timing information. It may also include `verification_url_qrcode`.
2. Show the user the `verification_url` and `user_code`. The user opens the URL and follows the approval flow, entering the code if prompted. Alternatively, render the approval URL as a QR code for scanning with the Xunlei app.
3. Poll `POST https://xluser-ssl.xunlei.com/v1/auth/token` with `grant_type=urn:ietf:params:oauth:grant-type:device_code` and the `device_code` until the user approves or the code expires.
4. When supported by the integration's credentials, use the same token endpoint with `grant_type=refresh_token` to renew access.

`Client::login_interactive()` handles this sequence. It prints the verification URL, user code, and terminal QR code to stderr, respects the server's polling interval, adds five seconds when the server returns `slow_down`, and stops on expiration or denial. It prefers `verification_url_qrcode` for the QR image when available.

### Client credentials

Choose either credentials for your own registered integration:

```rust
use thunder_task_rs::auth::ClientCredentials;

let credentials = ClientCredentials::new("your-client-id", "your-client-secret");
```

Or the public credentials embedded in the official NAS client (`pan-xunlei-com`):

```rust
use thunder_task_rs::auth::ClientCredentials;

let credentials = ClientCredentials::nas_client();
assert_eq!(credentials.client_id, "X9ibISwpIp8jQ4Ya");
assert!(credentials.client_secret.is_empty());
```

The NAS client ID is publicly shipped in the official client. Its device-flow login with an empty client secret has been verified working against the live endpoint. The account owner still approves each login. The library omits the optional scope parameter so the server applies the client's default grant.

**Refresh limitation:** the current implementation requires a non-empty client secret for refresh, including automatic refresh after HTTP 401. `nas_client()` therefore supports device-flow login but cannot refresh through the current refresh implementation. Repeat device-flow login when its access token expires. Registered integrations with suitable credentials can use refresh normally.

### Interactive login with a terminal QR code

```rust
use thunder_task_rs::{auth::ClientCredentials, Client};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = Client::new(ClientCredentials::nas_client());

    // Prints the URL, user code, and QR code, then waits for approval.
    let session = client.login_interactive().await?;

    // Persist this JSON in your application's secure session storage.
    let session_json = serde_json::to_string_pretty(&session)?;
    // Save session_json securely; it contains credentials and bearer tokens.
    let _ = session_json;
    Ok(())
}
```

For a custom login interface, use `auth::device_code`, `auth::poll_token`, and `auth::qr_terminal` directly. For example, to render the approval prompt yourself:

```rust
use thunder_task_rs::auth::{self, DeviceCode};

fn print_approval(code: &DeviceCode) -> thunder_task_rs::Result<()> {
    let qr_url = if code.verification_url_qrcode.is_empty() {
        &code.verification_url
    } else {
        &code.verification_url_qrcode
    };
    println!("Open {} and use code {}", code.verification_url, code.user_code);
    println!("{}", auth::qr_terminal(qr_url)?);
    Ok(())
}
```

`poll_token` returns `Ok(None)` for both `authorization_pending` and `slow_down`. A custom poller must manage the interval and expiration itself, increasing the delay on slow-down; prefer `login_interactive()` for automatic handling of these distinct outcomes.

### Restore a session from JSON

`Session` serializes both client credentials and tokens. Store it securely and keep it out of source control and logs.

```rust
use thunder_task_rs::{auth::Session, Client};

fn restore_client(session_json: &str) -> Result<Client, serde_json::Error> {
    let session: Session = serde_json::from_str(session_json)?;
    Ok(Client::from_session(session))
}
```

Authenticated requests retry once after HTTP 401 if token refresh succeeds. After requests that may refresh tokens, serialize the latest session from `client.session()` and update your stored JSON. Refresh failure returns `Error::NotAuthenticated`.

## API coverage

Drive paths below use `https://api-gateway-pan.xunlei.com`; authentication paths use `https://xluser-ssl.xunlei.com`.

| Area | Rust API | Endpoint and behavior |
| --- | --- | --- |
| Login: device code | `auth::device_code` | `POST /v1/auth/device/code` |
| Login: token polling | `auth::poll_token`, `Client::login_interactive` | `POST /v1/auth/token`, device-code grant |
| Login: refresh | `auth::refresh`, automatic retry on HTTP 401 | `POST /v1/auth/token`, refresh-token grant; requires a non-empty secret in this implementation |
| Authorized devices | `Client::devices` | `GET /xlppc/drive/v1/devices`, falling back to auth-host `GET /v1/user/authorized/devices` on errors other than `NotAuthenticated` |
| Create task | `Client::create_task` | `POST /xlppc/drive/v1/task` |
| List tasks | `Client::tasks` | `GET /xlppc/drive/v1/tasks`, with phase and pagination parameters |
| Get task | `Client::task` | `GET /xlppc/drive/v1/tasks/{id}` |
| Link analysis | `Client::analyze_link` | `POST /xlppc/drive/v1/resource/list`, with `urls` and `space`; returns resource metadata/tree as JSON |
| Remote download progress | `Client::tasks(TaskFilter)` | Task list with a phase filter; inspect each task's `progress` and `phase` |
| Pause / resume / set phase | `Client::pause_task`, `resume_task`, `operate_task` | `PATCH /xlppc/drive/v1/task/{id}`; pause/resume first fetch the task to discover its space |
| Delete tasks | `Client::delete_tasks` | `DELETE /xlppc/drive/v1/tasks`, with repeated `task_ids` query parameters |

## Devices, downloads, and link analysis

The following example takes an authenticated `Client`, lists authorized devices, analyzes a link, and creates a download on one selected device. Replace the example URL with your own download link and choose the intended device in your application.

```rust
use thunder_task_rs::{types::CreateTaskRequest, Client};

async fn submit_download(client: &mut Client) -> Result<(), Box<dyn std::error::Error>> {
    let devices = client.devices().await?;
    for device in &devices {
        println!("{}: {} (space: {})", device.id, device.name, device.space);
    }

    let device = devices.first().ok_or("no authorized devices")?;
    let device_space = device.space.clone();
    let url = "https://example.com/archive.zip";

    let resources = client.analyze_link(url, &device_space).await?;
    println!("{}", serde_json::to_string_pretty(&resources)?);

    let task = client
        .create_task(CreateTaskRequest::new(url).with_space(device_space))
        .await?;
    println!("Created task {}: {}", task.id, task.name);
    Ok(())
}
```

Use `Device::space` to route a task to a device. If the server omits a device's space, `devices()` fills it from the device ID. A request without `.with_space(...)` uses `SPACE_USER`, the empty string representing the personal cloud drive.

### Query remote download progress

```rust
use thunder_task_rs::{task::TaskFilter, types::Phase, Client};

async fn show_progress(client: &mut Client) -> thunder_task_rs::Result<()> {
    let tasks = client.tasks(TaskFilter {
        phases: vec![Phase::Pending, Phase::Running],
        limit: Some(100),
        ..Default::default()
    }).await?;

    for task in tasks {
        println!("{}: {:?}, progress: {:?}", task.id, task.phase, task.progress);
    }
    Ok(())
}
```

Use `TaskFilter::default()` to list without a phase filter. `tasks()` fetches one page and returns `Vec<Task>`; it does not automatically traverse pages or expose the next-page token. For full pagination, use `Client::raw_request` to read the response's `next_page_token`, then pass it through `TaskFilter::page_token` for a subsequent request. Progress is preserved as `Option<serde_json::Value>` in the server's representation; interpret it according to the returned data rather than assuming a fixed percentage scale.

### Pause, resume, and delete

```rust
use thunder_task_rs::{types::Phase, Client};

async fn manage_task(
    client: &mut Client,
    task_id: &str,
    device_space: &str,
) -> thunder_task_rs::Result<()> {
    let task = client.task(task_id).await?;
    println!("Current phase: {:?}", task.phase);

    client.pause_task(task_id).await?;
    client.resume_task(task_id).await?;

    // If you already know the space, set the phase directly.
    client.operate_task(task_id, device_space, Phase::Paused).await?;
    client.operate_task(task_id, device_space, Phase::Running).await?;

    client.delete_tasks(&[task_id.to_owned()]).await?;
    Ok(())
}
```

Run only the operations you intend; this example ends by deleting the task. `delete_tasks(&[])` is a no-op. Phase changes use PATCH with the target `space` and a JSON-encoded `set_params.spec` containing the requested phase.

## Types

### URL and file sources

`CreateTaskRequest` wraps a `TaskSource`:

- `TaskSource::Url(String)` submits a link such as HTTP(S), FTP, magnet, ed2k, or thunder. Construct it with `CreateTaskRequest::new(url)`.
- `TaskSource::File { file_id, sub_file_index }` references a file already uploaded to the drive, such as a torrent. Construct it with `CreateTaskRequest::from_file(file_id)`. The library does not upload the local torrent for you.

For a torrent with multiple files, analyze its resources and select the required zero-based sub-file indexes:

```rust
use thunder_task_rs::types::CreateTaskRequest;

let request = CreateTaskRequest::from_file("uploaded-torrent-file-id")
    .with_sub_file_index(vec![0, 2])
    .with_total_file_count(2)
    .with_mime_type("application/x-bittorrent")
    .with_space("target-device-space");
```

An empty `sub_file_index` selects all files. `with_sub_file_index` only affects file sources. Optional builders also set the display name, output filename, destination folder, and total file count.

### Task phases

| Rust variant | API value |
| --- | --- |
| `Phase::Pending` | `PHASE_TYPE_PENDING` |
| `Phase::Running` | `PHASE_TYPE_RUNNING` |
| `Phase::Paused` | `PHASE_TYPE_PAUSED` |
| `Phase::Error` | `PHASE_TYPE_ERROR` |
| `Phase::Complete` | `PHASE_TYPE_COMPLETE` |
| `Phase::Unknown(String)` | Preserves an unrecognized server value verbatim |

`Phase::as_str()` returns the wire value. `Task` and `Device` preserve unrecognized response fields in `extra`; numeric-looking task fields may arrive as either JSON strings or numbers.

## Configuration and errors

`ClientConfig` supports a custom `reqwest::Client` through `with_http` and custom drive/auth origins through `with_hosts`. `Client::raw_request` provides authenticated requests for drive endpoints without a dedicated wrapper.

Operations return `thunder_task_rs::Result<T>`. Errors distinguish HTTP failures, structured API errors, expired or denied login, missing authentication/configuration, and unexpected response shapes. Structured API errors are checked even when the HTTP status is 200.

## Scope of this library

This library operates on the user's own account through official Xunlei endpoints and the account owner's authorization. Xunlei's server-side quotas, VIP tiers, account permissions, and download limits apply unchanged. This library does not bypass them.

Downloads run on the selected authorized device or service. This crate controls tasks through the cloud API; your target device must be available and able to execute the requested download.

## Development

Run the test suite from the repository root:

```sh
cargo test
```

## License

MIT.
