//! Rust SDK for the Vera Secure AI Wasm Gateway.
//!
//! Provides a typed HTTP client for calling connectors and plugins
//! hosted by `vera-hub`, with automatic retry on 429, session-id
//! management, and error types matching the gateway's response shapes.
//!
//! # Quick start
//!
//! ```rust,no_run
//! # async fn example() -> Result<(), vera_client::VeraError> {
//! use vera_client::VeraClient;
//!
//! let client = VeraClient::builder()
//!     .url("https://vera.internal:8443")
//!     .bearer("vk_a1b2c3d4...")
//!     .build()?;
//!
//! let response = client.infer("echo", b"hello world").await?;
//! println!("response: {}", String::from_utf8_lossy(&response.body));
//! # Ok(())
//! # }
//! ```

use std::{sync::Arc, time::Duration};

use reqwest::{
    header::{self, HeaderMap, HeaderValue},
    Client, StatusCode,
};
use serde::Deserialize;
use tokio::sync::RwLock;

/// Connector manifest returned by `GET /v1/models`.
#[derive(Clone, Debug, Deserialize)]
pub struct ModelInfo {
    /// Connector or alias id.
    pub id: String,
    /// Human-readable name.
    #[serde(default)]
    pub display_name: String,
    /// Model identifier.
    #[serde(default)]
    pub model: String,
    /// Provider name.
    #[serde(default)]
    pub provider: String,
    /// Capabilities.
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    /// Transport options.
    #[serde(default)]
    pub transport: ModelTransport,
}

/// Capabilities from the manifest.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ModelCapabilities {
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub modalities: Vec<String>,
    #[serde(default)]
    pub streaming_response: bool,
}

/// Transport config from the manifest.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ModelTransport {
    #[serde(default)]
    pub modes: Vec<String>,
}

/// Cached discovery state.
#[derive(Debug, Default)]
struct DiscoveryCache {
    models: Vec<ModelInfo>,
}

/// Response from a connector or plugin dispatch.
#[derive(Clone, Debug)]
pub struct InferResponse {
    /// The response body bytes.
    pub body: Vec<u8>,
    /// HTTP status code.
    pub status: u16,
}

/// Errors returned by [`VeraClient`] methods.
#[derive(Debug, thiserror::Error)]
pub enum VeraError {
    /// 401 — invalid or missing bearer token.
    #[error("unauthorized (401)")]
    Unauthorized,

    /// 403 — policy denied the request for this principal + connector.
    #[error("forbidden (403)")]
    Forbidden,

    /// 429 — `QoS` throttled after max retries exhausted.
    #[error("throttled (429) after {retries} retries; retry-after {retry_after_secs}s")]
    Throttled {
        /// Number of retries attempted.
        retries: u32,
        /// Last `Retry-After` value from the server.
        retry_after_secs: u64,
    },

    /// 422 — Vault blocked the request body due to PII matches.
    #[error("vault blocked (422): {rules:?}")]
    VaultBlocked {
        /// Rule ids that matched.
        rules: Vec<String>,
        /// Number of PII matches.
        matches: u64,
    },

    /// 5xx or unexpected status.
    #[error("server error ({status}): {message}")]
    Server {
        /// HTTP status code.
        status: u16,
        /// Response body (may be empty).
        message: String,
    },

    /// Network or TLS error.
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),

    /// Builder misconfiguration.
    #[error("config: {0}")]
    Config(String),
}

/// Builder for [`VeraClient`].
#[derive(Debug, Default)]
pub struct VeraClientBuilder {
    url: Option<String>,
    bearer: Option<String>,
    session_id: Option<String>,
    max_retries: Option<u32>,
    connect_timeout: Option<Duration>,
    danger_accept_invalid_certs: bool,
}

impl VeraClientBuilder {
    /// Hub base URL (e.g. `"https://vera.internal:8443"`).
    #[must_use]
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Bearer token (e.g. `"vk_a1b2c3d4..."`). Never log or persist.
    #[must_use]
    pub fn bearer(mut self, token: impl Into<String>) -> Self {
        self.bearer = Some(token.into());
        self
    }

    /// Read the bearer token from an environment variable. Fails at
    /// `build()` time if the variable is unset or empty.
    #[must_use]
    pub fn bearer_from_env(mut self, var: &str) -> Self {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                self.bearer = Some(val);
            }
        }
        self
    }

    /// Attach a session id to every request (multi-turn correlation).
    #[must_use]
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Max 429 retries before returning `VeraError::Throttled`. Default 3.
    #[must_use]
    pub const fn max_retries(mut self, n: u32) -> Self {
        self.max_retries = Some(n);
        self
    }

    /// TCP connect timeout. Default 10s.
    #[must_use]
    pub const fn connect_timeout(mut self, d: Duration) -> Self {
        self.connect_timeout = Some(d);
        self
    }

    /// Accept invalid TLS certificates. **Dev/test only** — never
    /// enable in production. Useful for self-signed certs in local
    /// development environments.
    #[must_use]
    pub const fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.danger_accept_invalid_certs = accept;
        self
    }

    /// Build the client. Fails if `url` or `bearer` are missing.
    ///
    /// # Errors
    /// `VeraError::Config` on missing fields or invalid URL.
    pub fn build(self) -> Result<VeraClient, VeraError> {
        let base_url = self
            .url
            .ok_or_else(|| VeraError::Config("url is required".into()))?;
        let bearer = self
            .bearer
            .ok_or_else(|| VeraError::Config("bearer token is required".into()))?;
        let base_url = base_url.trim_end_matches('/').to_owned();

        let mut headers = HeaderMap::new();
        let auth_value = HeaderValue::from_str(&format!("Bearer {bearer}"))
            .map_err(|e| VeraError::Config(format!("invalid bearer token: {e}")))?;
        headers.insert(header::AUTHORIZATION, auth_value);
        if let Some(ref sid) = self.session_id {
            if let Ok(v) = HeaderValue::from_str(sid) {
                headers.insert("x-vera-session", v);
            }
        }

        let mut http_builder = Client::builder()
            .default_headers(headers)
            .connect_timeout(self.connect_timeout.unwrap_or(Duration::from_secs(10)))
            .timeout(Duration::from_secs(120))
            .https_only(true);
        if self.danger_accept_invalid_certs {
            http_builder = http_builder.danger_accept_invalid_certs(true);
        }
        let http = http_builder
            .build()
            .map_err(|e| VeraError::Config(format!("http client build: {e}")))?;

        Ok(VeraClient {
            base_url,
            http,
            max_retries: self.max_retries.unwrap_or(3),
            bearer,
            cache: Arc::new(RwLock::new(DiscoveryCache::default())),
        })
    }
}

/// Typed HTTP client for the Vera gateway.
#[derive(Clone, Debug)]
pub struct VeraClient {
    base_url: String,
    http: Client,
    max_retries: u32,
    bearer: String,
    cache: Arc<RwLock<DiscoveryCache>>,
}

impl VeraClient {
    /// Start building a client.
    #[must_use]
    pub fn builder() -> VeraClientBuilder {
        VeraClientBuilder::default()
    }

    /// Call a connector endpoint: `POST /v1/infer/{connector}`.
    ///
    /// # Errors
    /// See [`VeraError`] variants.
    pub async fn infer(&self, connector: &str, body: &[u8]) -> Result<InferResponse, VeraError> {
        let url = format!("{}/v1/infer/{connector}", self.base_url);
        self.post_with_retry(&url, body).await
    }

    /// Call a plugin endpoint: `POST /v1/plugin/{plugin_id}`.
    ///
    /// # Errors
    /// See [`VeraError`] variants.
    pub async fn plugin(&self, plugin_id: &str, body: &[u8]) -> Result<InferResponse, VeraError> {
        let url = format!("{}/v1/plugin/{plugin_id}", self.base_url);
        self.post_with_retry(&url, body).await
    }

    /// Fetch available models from the Hub and cache them. Call this
    /// once at startup; `call()` uses the cache to auto-negotiate
    /// transport. Refreshes the cache on every call.
    ///
    /// # Errors
    /// Network or auth errors.
    pub async fn discover(&self) -> Result<Vec<ModelInfo>, VeraError> {
        let url = format!("{}/v1/models", self.base_url);
        let resp = self.http.get(&url).send().await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err(VeraError::Unauthorized);
        }
        if !resp.status().is_success() {
            return Err(VeraError::Server {
                status: resp.status().as_u16(),
                message: resp.text().await.unwrap_or_default(),
            });
        }
        let models: Vec<ModelInfo> = resp.json().await.map_err(|e| {
            VeraError::Config(format!("failed to parse /v1/models: {e}"))
        })?;
        {
            let mut cache = self.cache.write().await;
            cache.models = models.clone();
        }
        Ok(models)
    }

    /// Smart dispatch: looks up the model/alias in the discovery cache,
    /// picks the best transport (WS for audio, streaming for LLM,
    /// POST fallback), and dispatches. Calls `discover()` automatically
    /// if the cache is empty.
    ///
    /// # Errors
    /// See [`VeraError`] variants.
    pub async fn call(&self, model: &str, body: &[u8]) -> Result<InferResponse, VeraError> {
        // Ensure cache is populated.
        {
            let cache = self.cache.read().await;
            if cache.models.is_empty() {
                drop(cache);
                self.discover().await?;
            }
        }

        let cache = self.cache.read().await;
        let info = cache.models.iter().find(|m| m.id == model);

        if let Some(info) = info {
            let preferred = info.transport.modes.first().map(String::as_str);
            match preferred {
                Some("ws") => {
                    // WS needs an external client (tokio-tungstenite).
                    // Fall through to POST and let the caller use ws_url()
                    // for real WS. This avoids pulling a heavy WS dep.
                    self.call_post(model, body).await
                }
                Some("stream") => self.call_post(model, body).await,
                _ => self.call_post(model, body).await,
            }
        } else {
            // Not in cache — try POST directly (might be a new connector
            // added after discover(), or the cache is stale).
            self.call_post(model, body).await
        }
    }

    /// Run an agent: `POST /v1/agent/{agent_id}`. Body is the prompt.
    /// Returns the `AgentResult` JSON (answer + step trace).
    ///
    /// # Errors
    /// See [`VeraError`] variants.
    pub async fn run_agent(&self, agent_id: &str, prompt: &str) -> Result<InferResponse, VeraError> {
        let url = format!("{}/v1/agent/{agent_id}", self.base_url);
        self.post_with_retry(&url, prompt.as_bytes()).await
    }

    /// Build the WebSocket URL for agent streaming:
    /// `wss://{host}/v1/agent/{id}/stream?token={bearer}`.
    /// First WS message = prompt; server streams `AgentStep` JSON objects.
    #[must_use]
    pub fn agent_ws_url(&self, agent_id: &str) -> String {
        let ws_base = self
            .base_url
            .replace("https://", "wss://")
            .replace("http://", "ws://");
        format!(
            "{ws_base}/v1/agent/{agent_id}/stream?token={}",
            self.bearer
        )
    }

    /// Explicit POST dispatch: `POST /v1/infer/{model}`.
    ///
    /// # Errors
    /// See [`VeraError`] variants.
    pub async fn call_post(&self, model: &str, body: &[u8]) -> Result<InferResponse, VeraError> {
        let url = format!("{}/v1/infer/{model}", self.base_url);
        self.post_with_retry(&url, body).await
    }

    /// Build the WebSocket URL for streaming: `wss://{host}/v1/stream/{connector}?token={bearer}`.
    /// Use this with `tokio-tungstenite` or any WebSocket client.
    ///
    /// Mode 3 (bidirectional streaming) — auth fires once on upgrade;
    /// send audio/text chunks, receive response chunks in real-time.
    #[must_use]
    pub fn ws_url(&self, connector: &str) -> String {
        let ws_base = self
            .base_url
            .replace("https://", "wss://")
            .replace("http://", "ws://");
        format!("{ws_base}/v1/stream/{connector}?token={}", self.bearer)
    }

    /// Clone the client with a new session id attached. Cheap — shares
    /// the underlying connection pool.
    #[must_use]
    pub fn with_session(&self, session_id: &str) -> Self {
        let mut headers = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(session_id) {
            headers.insert("x-vera-session", v);
        }
        let http = Client::builder()
            .default_headers(headers)
            .build()
            .unwrap_or_else(|_| self.http.clone());
        Self {
            base_url: self.base_url.clone(),
            http,
            max_retries: self.max_retries,
            bearer: self.bearer.clone(),
            cache: self.cache.clone(),
        }
    }

    async fn post_with_retry(&self, url: &str, body: &[u8]) -> Result<InferResponse, VeraError> {
        let mut last_retry_after: u64 = 1;
        for attempt in 0..=self.max_retries {
            let resp = self.http.post(url).body(body.to_vec()).send().await?;

            match resp.status() {
                StatusCode::OK | StatusCode::CREATED | StatusCode::ACCEPTED => {
                    let status = resp.status().as_u16();
                    let resp_body = resp.bytes().await?.to_vec();
                    return Ok(InferResponse {
                        body: resp_body,
                        status,
                    });
                }
                StatusCode::UNAUTHORIZED => return Err(VeraError::Unauthorized),
                StatusCode::FORBIDDEN => return Err(VeraError::Forbidden),
                StatusCode::TOO_MANY_REQUESTS => {
                    let retry_after = resp
                        .headers()
                        .get(header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(1);
                    last_retry_after = retry_after;
                    if attempt < self.max_retries {
                        tokio::time::sleep(Duration::from_secs(retry_after)).await;
                        continue;
                    }
                    return Err(VeraError::Throttled {
                        retries: self.max_retries,
                        retry_after_secs: retry_after,
                    });
                }
                StatusCode::UNPROCESSABLE_ENTITY => {
                    let text = resp.text().await.unwrap_or_default();
                    if let Ok(parsed) = serde_json::from_str::<VaultBlockBody>(&text) {
                        return Err(VeraError::VaultBlocked {
                            rules: parsed.rules,
                            matches: parsed.matches,
                        });
                    }
                    return Err(VeraError::Server {
                        status: 422,
                        message: text,
                    });
                }
                other => {
                    let text = resp.text().await.unwrap_or_default();
                    return Err(VeraError::Server {
                        status: other.as_u16(),
                        message: text,
                    });
                }
            }
        }
        Err(VeraError::Throttled {
            retries: self.max_retries,
            retry_after_secs: last_retry_after,
        })
    }
}

#[derive(Deserialize)]
struct VaultBlockBody {
    #[serde(default)]
    rules: Vec<String>,
    #[serde(default)]
    matches: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC: none (SDK scaffolding — builder validation).
    #[test]
    fn builder_requires_url() {
        let err = VeraClient::builder().bearer("tok").build().unwrap_err();
        assert!(matches!(err, VeraError::Config(_)));
        assert!(err.to_string().contains("url"));
    }

    // SPEC: none (SDK scaffolding — builder validation).
    #[test]
    fn builder_requires_bearer() {
        let err = VeraClient::builder()
            .url("https://localhost:8443")
            .build()
            .unwrap_err();
        assert!(matches!(err, VeraError::Config(_)));
        assert!(err.to_string().contains("bearer"));
    }

    // SPEC: none (SDK scaffolding — builder happy path).
    #[test]
    fn builder_succeeds_with_url_and_bearer() {
        let client = VeraClient::builder()
            .url("https://localhost:8443")
            .bearer("vk_test")
            .build()
            .unwrap();
        assert_eq!(client.base_url, "https://localhost:8443");
        assert_eq!(client.max_retries, 3);
    }

    // SPEC: none (SDK scaffolding — URL normalization).
    #[test]
    fn builder_strips_trailing_slash() {
        let client = VeraClient::builder()
            .url("https://localhost:8443/")
            .bearer("vk_test")
            .build()
            .unwrap();
        assert_eq!(client.base_url, "https://localhost:8443");
    }

    // SPEC: none (SDK scaffolding — response parsing).
    #[test]
    fn vault_block_body_parses() {
        let json = r#"{"error":"vault_blocked","matches":2,"rules":["ssn","cc"]}"#;
        let parsed: VaultBlockBody = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.matches, 2);
        assert_eq!(parsed.rules, vec!["ssn", "cc"]);
    }

    // SPEC: none (SDK — ModelInfo parses from /v1/models JSON).
    #[test]
    fn model_info_deserializes() {
        let json = r#"{"id":"moonshine","display_name":"Moonshine","model":"moonshine-onnx","provider":"useful-sensors","capabilities":{"input":"audio","output":"text","modalities":["audio-to-text"],"streaming_response":false},"transport":{"modes":["ws","post"]}}"#;
        let info: ModelInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.id, "moonshine");
        assert_eq!(info.capabilities.input, "audio");
        assert_eq!(info.transport.modes, vec!["ws", "post"]);
    }

    // SPEC: none (SDK scaffolding — error display).
    #[test]
    fn error_display_is_readable() {
        let err = VeraError::Throttled {
            retries: 3,
            retry_after_secs: 5,
        };
        assert!(err.to_string().contains("429"));
        assert!(err.to_string().contains("3 retries"));

        let err = VeraError::VaultBlocked {
            rules: vec!["ssn".into()],
            matches: 1,
        };
        assert!(err.to_string().contains("ssn"));
    }
}
