//! Configurable authenticated client and interactive OAuth login.

use reqwest::{Method, StatusCode};
use serde_json::Value;
use tokio::time::{Duration, Instant};
use url::Url;

use crate::auth::{self, ClientCredentials, PollResult, Session, TokenSet};
use crate::{Error, Result, AUTH_HOST, DRIVE_HOST};

/// Transport, credentials, endpoints, and optional initial tokens.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Caller-supplied application credentials.
    pub credentials: ClientCredentials,
    /// Reusable asynchronous HTTP transport.
    pub http: reqwest::Client,
    /// Drive API origin.
    pub drive_host: Url,
    /// OAuth and authorized-device API origin.
    pub auth_host: Url,
    /// Tokens restored from a previous login, if any.
    pub tokens: Option<TokenSet>,
}

impl ClientConfig {
    /// Configure the official endpoints with caller-provided credentials.
    pub fn new(credentials: ClientCredentials) -> Self {
        Self {
            credentials,
            http: reqwest::Client::new(),
            drive_host: Url::parse(DRIVE_HOST).expect("static drive URL"),
            auth_host: Url::parse(AUTH_HOST).expect("static auth URL"),
            tokens: None,
        }
    }

    /// Use a custom HTTP transport.
    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// Override the drive and auth origins, in that order.
    pub fn with_hosts(mut self, drive_host: Url, auth_host: Url) -> Self {
        self.drive_host = drive_host;
        self.auth_host = auth_host;
        self
    }
}

/// Stateful API client. Mutable calls keep refreshed session tokens available.
#[derive(Debug, Clone)]
pub struct Client {
    config: ClientConfig,
    session: Option<Session>,
}

impl Client {
    /// Create an unauthenticated client using the official API origins.
    pub fn new(credentials: ClientCredentials) -> Self {
        Self::from_config(ClientConfig::new(credentials))
    }

    /// Create a client with custom transport, origins, and optional tokens.
    pub fn from_config(config: ClientConfig) -> Self {
        let session = config.tokens.clone().map(|tokens| Session {
            credentials: config.credentials.clone(),
            tokens,
        });
        Self { config, session }
    }

    /// Restore credentials and tokens from a saved session.
    pub fn from_session(session: Session) -> Self {
        let mut config = ClientConfig::new(session.credentials.clone());
        config.tokens = Some(session.tokens);
        Self::from_config(config)
    }

    /// Borrow the current session, including any refreshed tokens.
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// Print an approval prompt and QR code to stderr, then await authorization.
    ///
    /// Polling observes the server interval, adds five seconds on each slow-down,
    /// and stops when the device code expires. Persist the returned session securely.
    pub async fn login_interactive(&mut self) -> Result<Session> {
        let code = auth::device_code_at(
            &self.config.http,
            &self.config.credentials,
            &self.config.auth_host,
        )
        .await?;
        let url = if code.verification_url_qrcode.is_empty() {
            &code.verification_url
        } else {
            &code.verification_url_qrcode
        };
        let verification_url = if code.verification_url.is_empty() {
            url
        } else {
            &code.verification_url
        };
        let qr = auth::qr_terminal(url)?;
        let lifetime = Duration::from_secs(if code.expires_in == 0 {
            600
        } else {
            code.expires_in
        });
        let deadline = Instant::now()
            .checked_add(lifetime)
            .ok_or_else(|| Error::UnexpectedResponse("device-code lifetime is too large".into()))?;
        eprintln!(
            "Open {verification_url}\nUser code: {}\n{qr}",
            code.user_code
        );
        let mut interval = if code.interval == 0 { 5 } else { code.interval };
        let tokens = tokio::time::timeout_at(deadline, async {
            loop {
                tokio::time::sleep(Duration::from_secs(interval)).await;
                match auth::poll_token_at(
                    &self.config.http,
                    &self.config.credentials,
                    &code.device_code,
                    &self.config.auth_host,
                )
                .await?
                {
                    PollResult::Tokens(tokens) => return Ok::<TokenSet, Error>(tokens),
                    PollResult::Pending => {}
                    PollResult::SlowDown => interval = interval.saturating_add(5),
                }
            }
        })
        .await
        .map_err(|_| Error::LoginExpired)??;
        let session = Session {
            credentials: self.config.credentials.clone(),
            tokens,
        };
        self.config.tokens = Some(session.tokens.clone());
        self.session = Some(session.clone());
        Ok(session)
    }

    /// Send an authenticated drive request with an optional JSON body.
    ///
    /// `path` is origin-relative and may contain an already-encoded query.
    /// A 401 triggers at most one token refresh and one retry. Empty success
    /// bodies return JSON null; structured API errors are checked even on HTTP 200.
    pub async fn raw_request(
        &mut self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        self.authed_request(method, path, body).await
    }

    pub(crate) async fn authed_request(
        &mut self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let url = self.drive_url(path)?;
        self.request_url(method, url, body).await
    }

    pub(crate) fn drive_url(&self, path: &str) -> Result<Url> {
        if !path.starts_with('/') || path.starts_with("//") || path.contains('\\') {
            return Err(Error::UnexpectedResponse(
                "drive path must be origin-relative".into(),
            ));
        }
        auth::endpoint(&self.config.drive_host, path)
    }

    pub(crate) fn authorized_devices_url(&self) -> Result<Url> {
        auth::endpoint(&self.config.auth_host, "/v1/user/authorized/devices")
    }

    pub(crate) async fn request_url(
        &mut self,
        method: Method,
        url: Url,
        body: Option<Value>,
    ) -> Result<Value> {
        for attempt in 0..2 {
            let session = self
                .session
                .as_ref()
                .filter(|session| !session.tokens.access_token.is_empty())
                .ok_or(Error::NotAuthenticated)?;
            let mut request = self
                .config
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(&session.tokens.access_token);
            if let Some(body) = &body {
                request = request.json(body);
            }
            let response = request.send().await?;
            if response.status() != StatusCode::UNAUTHORIZED {
                return auth::response_value(response).await;
            }
            if attempt == 1 {
                return Err(Error::NotAuthenticated);
            }
            let refresh_token = session.tokens.refresh_token.clone();
            if refresh_token.is_empty() {
                return Err(Error::NotAuthenticated);
            }
            let mut tokens = auth::refresh_at(
                &self.config.http,
                &self.config.credentials,
                &refresh_token,
                &self.config.auth_host,
            )
            .await
            .map_err(|_| Error::NotAuthenticated)?;
            if tokens.access_token.is_empty() {
                return Err(Error::NotAuthenticated);
            }
            if tokens.refresh_token.is_empty() {
                tokens.refresh_token = refresh_token;
            }
            self.config.tokens = Some(tokens.clone());
            self.session = Some(Session {
                credentials: self.config.credentials.clone(),
                tokens,
            });
        }
        Err(Error::NotAuthenticated)
    }
}

impl From<ClientConfig> for Client {
    fn from(config: ClientConfig) -> Self {
        Self::from_config(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn configuration_and_session_restoration() {
        let session: Session = serde_json::from_value(json!({
            "credentials": {"client_id": "id", "client_secret": "secret"},
            "tokens": {"token_type": "Bearer", "access_token": "access", "refresh_token": "refresh"}
        }))
        .unwrap();
        let restored = Client::from_session(session.clone());
        assert_eq!(restored.session().unwrap().tokens.access_token, "access");
        let drive_host = Url::parse("https://drive.example.org").unwrap();
        let auth_host = Url::parse("https://auth.example.org").unwrap();
        let mut config = ClientConfig::new(session.credentials)
            .with_http(reqwest::Client::new())
            .with_hosts(drive_host, auth_host);
        config.tokens = Some(session.tokens);
        let client = Client::from(config);
        assert_eq!(
            client.drive_url("/tasks").unwrap().as_str(),
            "https://drive.example.org/tasks"
        );
        assert_eq!(
            client.authorized_devices_url().unwrap().as_str(),
            "https://auth.example.org/v1/user/authorized/devices"
        );
        assert_eq!(client.session().unwrap().tokens.refresh_token, "refresh");
        for path in [
            "https://other.example.org",
            "//other.example.org",
            "/\\other.example.org",
        ] {
            assert!(client.drive_url(path).is_err());
        }
    }

    #[tokio::test]
    async fn unauthenticated_calls_fail_without_network_access() {
        let mut client = Client::new(ClientCredentials::new("id", "secret"));
        assert!(client.session().is_none());
        assert!(matches!(
            client.raw_request(Method::GET, "/tasks", None).await,
            Err(Error::NotAuthenticated)
        ));
        assert!(matches!(
            client.devices().await,
            Err(Error::NotAuthenticated)
        ));
        assert!(matches!(
            client
                .create_task(crate::types::CreateTaskRequest::new(
                    "https://example.org/file"
                ))
                .await,
            Err(Error::NotAuthenticated)
        ));
    }
}
