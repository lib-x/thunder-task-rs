//! OAuth device-flow login for the Xunlei Pan cloud API.
//!
//! The flow (RFC 8628 device authorization grant, as used by the official
//! clients):
//!
//! 1. POST `/v1/auth/device/code` with the client credentials →
//!    `device_code`, `user_code`, `verification_url`
//! 2. The user opens `verification_url` (or scans the QR code that encodes
//!    it) in the Xunlei app / web page and approves
//! 3. Poll POST `/v1/auth/token` with `grant_type=
//!    "urn:ietf:params:oauth:grant-type:device_code"` until it returns
//!    `access_token` / `refresh_token`
//! 4. Later, refresh with `grant_type=refresh_token`

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{Error, Result};
use crate::AUTH_HOST;

/// Credentials identifying a client application. The official clients embed
/// public client ids; provide the one appropriate for your integration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClientCredentials {
    /// Application identifier supplied by the caller.
    pub client_id: String,
    /// Application secret supplied by the caller.
    pub client_secret: String,
}

impl ClientCredentials {
    /// Create credentials for your registered integration; no defaults are provided.
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
        }
    }

    /// Credentials embedded in the official Xunlei NAS client
    /// (`pan-xunlei-com`). The id is publicly shipped inside the client
    /// binary; the device-flow token endpoint accepts it with an empty
    /// secret. Use these only with the account owner's own consent.
    pub fn nas_client() -> Self {
        Self {
            client_id: "X9ibISwpIp8jQ4Ya".into(),
            client_secret: String::new(),
        }
    }
}

/// The pending device-code session returned by step 1.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    /// Opaque code used by the token polling endpoint.
    pub device_code: String,
    /// Short code displayed to the user.
    pub user_code: String,
    /// URL at which the user approves the login.
    #[serde(default)]
    pub verification_url: String,
    /// QR-specific approval URL, when supplied.
    #[serde(default)]
    pub verification_url_qrcode: String,
    /// Lifetime of the device code, in seconds.
    #[serde(default)]
    pub expires_in: u64,
    /// Minimum polling interval, in seconds.
    #[serde(default)]
    pub interval: u64,
}

/// Token bundle returned by a successful login.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TokenSet {
    /// OAuth token type.
    pub token_type: String,
    /// Bearer access token.
    pub access_token: String,
    /// Refresh token, if issued.
    #[serde(default)]
    pub refresh_token: String,
    /// Access-token lifetime, in seconds.
    #[serde(default)]
    pub expires_in: u64,
    /// Additional token response fields.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Persisted login state: keep this if you want sessions to survive restarts.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Session {
    /// Credentials needed for refresh; store the session securely.
    pub credentials: ClientCredentials,
    /// Current access and refresh tokens.
    pub tokens: TokenSet,
}

fn default_host() -> Url {
    Url::parse(AUTH_HOST).expect("static auth URL")
}

/// Request a device-code login session.
pub async fn device_code(http: &reqwest::Client, cred: &ClientCredentials) -> Result<DeviceCode> {
    device_code_at(http, cred, &default_host(), None).await
}

pub(crate) async fn device_code_at(
    http: &reqwest::Client,
    cred: &ClientCredentials,
    host: &Url,
    scope: Option<&str>,
) -> Result<DeviceCode> {
    validate_credentials(cred)?;
    // The NAS client id has no user.profile scope; omitting scope lets the
    // server apply the client's default grant (pan file/task access), which
    // is what download control needs.
    let mut form = vec![("client_id", cred.client_id.as_str())];
    if let Some(scope) = scope {
        form.push(("scope", scope));
    }
    let resp = http
        .post(endpoint(host, "/v1/auth/device/code")?)
        .form(&form)
        .send()
        .await?;
    parse_json(resp).await
}

/// Poll the token endpoint once for the given device code.
///
/// Returns `Ok(None)` while the user has not approved yet (`authorization_pending`),
/// `Ok(Some(tokens))` after approval, and an error on expiry/denial.
/// `slow_down` also returns `Ok(None)` for compatibility; callers driving the
/// flow themselves must increase their polling interval. Prefer
/// [`crate::Client::login_interactive`] for automatic interval management.
pub async fn poll_token(
    http: &reqwest::Client,
    cred: &ClientCredentials,
    device_code: &str,
) -> Result<Option<TokenSet>> {
    match poll_token_at(http, cred, device_code, &default_host()).await? {
        PollResult::Tokens(tokens) => Ok(Some(tokens)),
        PollResult::Pending | PollResult::SlowDown => Ok(None),
    }
}

pub(crate) enum PollResult {
    Tokens(TokenSet),
    Pending,
    SlowDown,
}

pub(crate) async fn poll_token_at(
    http: &reqwest::Client,
    cred: &ClientCredentials,
    device_code: &str,
    host: &Url,
) -> Result<PollResult> {
    validate_credentials(cred)?;
    let resp = http
        .post(endpoint(host, "/v1/auth/token")?)
        .form(&[
            ("client_id", cred.client_id.as_str()),
            ("client_secret", cred.client_secret.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
        ])
        .send()
        .await?;

    let http_error = resp.error_for_status_ref().err();
    let bytes = resp.bytes().await?;
    let body: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| Error::UnexpectedResponse(error.to_string()))?;
    let result = poll_body(body)?;
    if matches!(result, PollResult::Tokens(_)) {
        if let Some(error) = http_error {
            return Err(error.into());
        }
    }
    Ok(result)
}

fn poll_body(body: serde_json::Value) -> Result<PollResult> {
    match body.get("error").and_then(serde_json::Value::as_str) {
        Some("authorization_pending") => Ok(PollResult::Pending),
        Some("slow_down") => Ok(PollResult::SlowDown),
        Some("expired_token" | "access_denied") => Err(Error::LoginExpired),
        _ => {
            check_api_error(&body)?;
            decode(body).map(PollResult::Tokens)
        }
    }
}

/// Exchange a refresh token for a fresh token set.
pub async fn refresh(
    http: &reqwest::Client,
    cred: &ClientCredentials,
    refresh_token: &str,
) -> Result<TokenSet> {
    refresh_at(http, cred, refresh_token, &default_host()).await
}

pub(crate) async fn refresh_at(
    http: &reqwest::Client,
    cred: &ClientCredentials,
    refresh_token: &str,
    host: &Url,
) -> Result<TokenSet> {
    validate_refresh_credentials(cred)?;
    let resp = http
        .post(endpoint(host, "/v1/auth/token")?)
        .form(&[
            ("client_id", cred.client_id.as_str()),
            ("client_secret", cred.client_secret.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await?;
    parse_json(resp).await
}

/// Render the verification URL as a terminal QR code (unicode blocks).
pub fn qr_terminal(verification_url: &str) -> Result<String> {
    let code = qrcode::QrCode::new(verification_url.as_bytes())
        .map_err(|e| Error::UnexpectedResponse(format!("qr encode: {e}")))?;
    let rendered = code
        .render::<char>()
        .dark_color('█')
        .light_color(' ')
        .quiet_zone(true)
        .module_dimensions(1, 1)
        .build();
    let rows: Vec<_> = rendered.lines().collect();
    let mut output = String::new();
    for pair in rows.chunks(2) {
        let top = pair[0].chars();
        let bottom = pair
            .get(1)
            .copied()
            .unwrap_or("")
            .chars()
            .chain(std::iter::repeat(' '));
        for (upper, lower) in top.zip(bottom) {
            output.push(match (upper == '█', lower == '█') {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        output.push('\n');
    }
    Ok(output)
}

pub(crate) async fn parse_json<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T> {
    decode(response_value(resp).await?)
}

pub(crate) async fn response_value(resp: reqwest::Response) -> Result<serde_json::Value> {
    let http_error = resp.error_for_status_ref().err();
    let bytes = resp.bytes().await?;
    let parsed = if bytes.is_empty() {
        Ok(serde_json::Value::Null)
    } else {
        serde_json::from_slice(&bytes)
    };
    if let Ok(body) = &parsed {
        check_api_error(body)?;
    }
    if let Some(error) = http_error {
        return Err(error.into());
    }
    parsed.map_err(|error| Error::UnexpectedResponse(error.to_string()))
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(body: serde_json::Value) -> Result<T> {
    serde_json::from_value(body).map_err(|error| Error::UnexpectedResponse(error.to_string()))
}

pub(crate) fn check_api_error(body: &serde_json::Value) -> Result<()> {
    if let Some(error) = body
        .get("error")
        .filter(|error| !error.is_null() && **error != false && **error != "" && **error != 0)
    {
        return Err(Error::Api {
            code: body
                .get("error_code")
                .filter(|code| !code.is_null())
                .or(Some(error))
                .map(|code| {
                    code.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| code.to_string())
                }),
            message: body
                .get("error_description")
                .or_else(|| body.get("message"))
                .or_else(|| body.get("error"))
                .and_then(|v| v.as_str())
                .unwrap_or("request failed")
                .to_string(),
        });
    }
    Ok(())
}

pub(crate) fn endpoint(host: &Url, path: &str) -> Result<Url> {
    host.join(path)
        .map_err(|error| Error::UnexpectedResponse(error.to_string()))
}

fn validate_credentials(credentials: &ClientCredentials) -> Result<()> {
    if credentials.client_id.is_empty() {
        return Err(Error::MissingConfig("client_id"));
    }
    // client_secret is optional: the device-flow endpoint accepts the NAS
    // client id with an empty secret.
    Ok(())
}

fn validate_refresh_credentials(credentials: &ClientCredentials) -> Result<()> {
    validate_credentials(credentials)?;
    if credentials.client_secret.is_empty() {
        return Err(Error::MissingConfig("client_secret"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn oauth_poll_outcomes_remain_distinct() {
        assert!(matches!(
            poll_body(json!({"error": "authorization_pending"})),
            Ok(PollResult::Pending)
        ));
        assert!(matches!(
            poll_body(json!({"error": "slow_down"})),
            Ok(PollResult::SlowDown)
        ));
        for error in ["expired_token", "access_denied"] {
            assert!(matches!(
                poll_body(json!({"error": error})),
                Err(Error::LoginExpired)
            ));
        }
        assert!(matches!(
            poll_body(json!({"error": "invalid_client"})),
            Err(Error::Api { .. })
        ));
        assert!(matches!(
            poll_body(json!({"token_type": "Bearer", "access_token": "token"})),
            Ok(PollResult::Tokens(_))
        ));
        assert!(matches!(
            poll_body(json!({"unexpected": true})),
            Err(Error::UnexpectedResponse(_))
        ));
    }

    #[test]
    fn api_errors_are_checked_independently_of_http_status() {
        let error = check_api_error(
            &json!({"error": "denied", "error_code": 123, "error_description": "not allowed"}),
        );
        assert!(
            matches!(error, Err(Error::Api { code: Some(code), message }) if code == "123" && message == "not allowed")
        );
        for body in [
            json!({}),
            json!({"error": null}),
            json!({"error": ""}),
            json!({"error": false}),
            json!({"error": 0}),
        ] {
            assert!(check_api_error(&body).is_ok());
        }
    }

    #[test]
    fn sessions_round_trip_credentials_and_extra_token_fields() {
        let session: Session = serde_json::from_value(json!({
            "credentials": {"client_id": "test-id", "client_secret": "test-secret"},
            "tokens": {"token_type": "Bearer", "access_token": "token", "custom": true}
        }))
        .unwrap();
        let restored: Session =
            serde_json::from_value(serde_json::to_value(session).unwrap()).unwrap();
        assert_eq!(restored.credentials.client_id, "test-id");
        assert_eq!(restored.credentials.client_secret, "test-secret");
        assert_eq!(restored.tokens.extra["custom"], true);
        assert!(restored.tokens.refresh_token.is_empty());
    }

    #[test]
    fn terminal_qr_preserves_every_module() {
        let url = "https://example.org/approve?code=123";
        let code = qrcode::QrCode::new(url.as_bytes()).unwrap();
        let reference = code
            .render::<char>()
            .dark_color('█')
            .light_color(' ')
            .quiet_zone(true)
            .module_dimensions(1, 1)
            .build();
        let mut unpacked = Vec::new();
        for row in qr_terminal(url).unwrap().lines() {
            let mut upper = String::new();
            let mut lower = String::new();
            for pixel in row.chars() {
                upper.push(if matches!(pixel, '█' | '▀') {
                    '█'
                } else {
                    ' '
                });
                lower.push(if matches!(pixel, '█' | '▄') {
                    '█'
                } else {
                    ' '
                });
            }
            unpacked.push(upper);
            unpacked.push(lower);
        }
        let expected: Vec<_> = reference.lines().collect();
        assert_eq!(&unpacked[..expected.len()], expected);
        assert!(qr_terminal(&"x".repeat(10_000)).is_err());
    }

    #[tokio::test]
    async fn missing_credentials_fail_before_network_access() {
        let http = reqwest::Client::new();
        assert!(matches!(
            device_code(&http, &ClientCredentials::new("", "secret")).await,
            Err(Error::MissingConfig("client_id"))
        ));
        assert!(matches!(
            refresh(&http, &ClientCredentials::new("id", ""), "refresh").await,
            Err(Error::MissingConfig("client_secret"))
        ));
    }
}
