//! LLM client port of `Services/LlmApiService.cs` — `POST {base}/chat/completions`
//! with `{model, messages, max_tokens: 2048}`, optional system message (CleanOutput),
//! Bearer + HTTP-Referer + X-Title headers, `choices[0].message.content` parse, 500-char
//! body truncation on errors, 60 s timeout, constructor errors for empty key/url/model.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

/// Error classes of the source, preserved 1:1 (message text may differ, class may not).
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("Request timed out after 60s\nEndpoint: {endpoint}")]
    Timeout { endpoint: String },

    #[error("Network error connecting to {endpoint}\nDetails: {details}")]
    Network { endpoint: String, details: String },

    #[error("Unexpected error calling API\nEndpoint: {endpoint}\nDetails: {details}")]
    Unexpected { endpoint: String, details: String },

    #[error("API returned {status} {reason}\n{detail}")]
    ApiStatus {
        status: u16,
        reason: String,
        detail: String,
    },

    #[error("{0}")]
    Parse(String),

    /// C# `ArgumentException` in the constructor (empty api key / base url / model).
    #[error("{0}")]
    InvalidConfig(String),
}

/// Port of `ILlmApiService`. Boxed-future form keeps the trait object-safe.
pub trait LlmClient: Send + Sync {
    fn call_api<'a>(
        &'a self,
        prompt: &'a str,
        system_prompt: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<String, LlmError>> + Send + 'a>>;
}

/// Port of `OpenAiApiService`. Constructor validates like the source's ArgumentExceptions.
pub struct OpenAiApiService {
    endpoint_url: String,
    model: String,
    auth_header: String,
}

/// Hand-written so the API key can never reach a trace or a panic message; every other
/// field is harmless to show.
impl std::fmt::Debug for OpenAiApiService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiApiService")
            .field("endpoint_url", &self.endpoint_url)
            .field("model", &self.model)
            .field("auth_header", &"<redacted>")
            .finish()
    }
}

impl OpenAiApiService {
    pub fn new(api_key: &str, base_url: &str, model: &str) -> Result<Self, LlmError> {
        // C# constructor validations, in source order. `IsNullOrWhiteSpace` is
        // `trim().is_empty()` here; the stored values keep the source's trims: only the
        // base URL's trailing slashes are dropped, only the key is trimmed.
        if api_key.trim().is_empty() {
            return Err(LlmError::InvalidConfig(
                "API key cannot be empty".to_string(),
            ));
        }
        if base_url.trim().is_empty() {
            return Err(LlmError::InvalidConfig(
                "Base URL cannot be empty".to_string(),
            ));
        }
        if model.trim().is_empty() {
            return Err(LlmError::InvalidConfig(
                "Model name cannot be empty".to_string(),
            ));
        }

        // rustls with the `rustls-no-provider` feature has no compiled-in process
        // default, so a provider must be installed before the first handshake.
        ensure_tls_provider();

        Ok(Self {
            endpoint_url: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            model: model.to_string(),
            auth_header: format!("Bearer {}", api_key.trim()),
        })
    }

    /// C# `OpenAiApiService.CallApiAsync`.
    ///
    /// The response body is always read before the status is inspected, exactly like the
    /// source: a body-read failure is its own error class, never a status error.
    pub async fn call_api(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
    ) -> Result<String, LlmError> {
        let request_body = ChatRequest::new(&self.model, prompt, system_prompt);

        let json = serde_json::to_string(&request_body).map_err(|error| LlmError::Unexpected {
            endpoint: self.endpoint_url.clone(),
            details: format!("Failed to serialize request body: {error}"),
        })?;

        // C# `StringContent(json, Encoding.UTF8, "application/json")` sends the charset
        // parameter, so the header is set explicitly rather than through `.json()`.
        let response = shared_client()
            .post(&self.endpoint_url)
            .header("Authorization", self.auth_header.as_str())
            .header("HTTP-Referer", HTTP_REFERER)
            .header("X-Title", X_TITLE)
            .header(reqwest::header::CONTENT_TYPE, CONTENT_TYPE_WITH_CHARSET)
            .body(json)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| map_send_error(&self.endpoint_url, error))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| LlmError::Unexpected {
                endpoint: self.endpoint_url.clone(),
                details: format!("Failed to read response body: {error}"),
            })?;

        if !status.is_success() {
            return Err(LlmError::ApiStatus {
                status: status.as_u16(),
                reason: status.canonical_reason().unwrap_or_default().to_string(),
                detail: extract_error_message(&body),
            });
        }

        parse_response(&body)
    }
}

impl LlmClient for OpenAiApiService {
    fn call_api<'a>(
        &'a self,
        prompt: &'a str,
        system_prompt: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<String, LlmError>> + Send + 'a>> {
        // Delegates to the inherent `call_api` above (the port of `CallApiAsync`); the
        // inherent method wins method resolution, so this is not recursive.
        Box::pin(async move { self.call_api(prompt, system_prompt).await })
    }
}

/// Installs the process-wide `ring` crypto provider for rustls.
///
/// `rustls-no-provider` leaves the process default unset, and the first handshake
/// panics without one. `install_default` fails with `AlreadyInstalled` when another
/// module installed a provider first, which is not an error for this port. Guarded by
/// a `OnceLock`, so the install happens once per process.
pub fn ensure_tls_provider() {
    static PROVIDER: OnceLock<()> = OnceLock::new();
    PROVIDER.get_or_init(|| {
        // The `AlreadyInstalled` payload carries nothing a caller can act on, so it is
        // not logged.
        if rustls::crypto::ring::default_provider()
            .install_default()
            .is_err()
        {
            tracing::debug!("rustls crypto provider was already installed");
        }
    });
}

/// C# `SharedHttpClient` — a single process-wide client with a 60 s timeout, so the
/// socket pool and its exhaustion behavior match the source's static instance.
static SHARED_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// C# `new HttpClient { Timeout = TimeSpan.FromSeconds(60) }`.
fn shared_client() -> &'static reqwest::Client {
    SHARED_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "reqwest client builder failed; using the default client");
                reqwest::Client::new()
            })
    })
}

/// C# `SharedHttpClient.Timeout` = 60 s, covering the send and the body read.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// C# `request.Headers.Add("HTTP-Referer", "https://github.com/orbit-app")`.
const HTTP_REFERER: &str = "https://github.com/orbit-app";
/// C# `request.Headers.Add("X-Title", "Orbital")`.
const X_TITLE: &str = "Orbital";
/// C# `max_tokens = 2048`.
const MAX_TOKENS: u32 = 2048;
/// C# `StringContent(json, Encoding.UTF8, "application/json")` sends this exact value.
const CONTENT_TYPE_WITH_CHARSET: &str = "application/json; charset=utf-8";
/// C# `responseBody.Substring(0, 500) + "..."` — three dots, as in the source.
const ERROR_BODY_TRUNCATE_CHARS: usize = 500;

/// C# request body — `{ model, messages, max_tokens }` with the system message first
/// when one is supplied, matching the two anonymous-object shapes in the source.
#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    max_tokens: u32,
}

impl<'a> ChatRequest<'a> {
    fn new(model: &'a str, prompt: &'a str, system_prompt: Option<&'a str>) -> Self {
        let mut messages = Vec::new();
        if let Some(system_prompt) = system_prompt {
            messages.push(ChatMessage {
                role: "system",
                content: system_prompt,
            });
        }
        messages.push(ChatMessage {
            role: "user",
            content: prompt,
        });
        Self {
            model,
            messages,
            max_tokens: MAX_TOKENS,
        }
    }
}

/// C# `new { role = "system", content = systemPrompt }` / `new { role = "user", content = prompt }`.
#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// C# send-failure mapping. `HttpRequestException` (connect / DNS / request-level
/// failures) becomes Network; everything else that is not a timeout becomes Unexpected.
fn map_send_error(endpoint: &str, error: reqwest::Error) -> LlmError {
    if error.is_timeout() {
        // The endpoint is not a secret and matches the source's message text.
        tracing::debug!(%error, "LLM request timed out");
        LlmError::Timeout {
            endpoint: endpoint.to_string(),
        }
    } else if error.is_connect() || error.is_request() || error.is_dns() {
        tracing::debug!(%error, "LLM request failed before a response was received");
        LlmError::Network {
            endpoint: endpoint.to_string(),
            details: error.to_string(),
        }
    } else {
        tracing::debug!(%error, "LLM request failed unexpectedly");
        LlmError::Unexpected {
            endpoint: endpoint.to_string(),
            details: error.to_string(),
        }
    }
}

/// C# `ExtractErrorMessage` — `error.message` when present and non-blank, otherwise the
/// raw body truncated at 500 UTF-16 units (the source's `string.Length` count) + "...".
fn extract_error_message(response_body: &str) -> String {
    if response_body.trim().is_empty() {
        return "No error details provided".to_string();
    }

    // C# `catch { /* If JSON parsing fails, return raw body */ }`
    if let Ok(document) = serde_json::from_str::<serde_json::Value>(response_body)
        && let Some(message) = document
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(|message| message.as_str())
        && !message.trim().is_empty()
    {
        return message.to_string();
    }

    // C# `responseBody.Length > 500 ? responseBody.Substring(0, 500) + "..." : responseBody`.
    // `Length` counts UTF-16 code units, so the cut is taken on the same measure.
    let mut units = response_body.encode_utf16();
    let prefix: Vec<u16> = units.by_ref().take(ERROR_BODY_TRUNCATE_CHARS).collect();
    if units.next().is_some() {
        return format!("{}...", String::from_utf16_lossy(&prefix));
    }
    response_body.to_string()
}

/// C# `ParseResponse` — `choices[0].message.content` with the source's exact messages.
fn parse_response(response_json: &str) -> Result<String, LlmError> {
    // C# `catch (JsonException ex) { throw new Exception($"Failed to parse API response as JSON: {ex.Message}") }`
    let document: serde_json::Value = serde_json::from_str(response_json).map_err(|error| {
        LlmError::Parse(format!("Failed to parse API response as JSON: {error}"))
    })?;

    let choices = document
        .get("choices")
        .ok_or_else(|| LlmError::Parse("Response missing 'choices' field".to_string()))?;

    // C# `choices.GetArrayLength() == 0`. A `choices` that is not an array makes that
    // call throw in the source; both cases end up as a parse failure here.
    let first_choice = choices
        .as_array()
        .filter(|items| !items.is_empty())
        .and_then(|items| items.first())
        .ok_or_else(|| LlmError::Parse("Response 'choices' array is empty".to_string()))?;

    let message = first_choice
        .get("message")
        .ok_or_else(|| LlmError::Parse("Response choice missing 'message' field".to_string()))?;

    let content = message
        .get("content")
        .ok_or_else(|| LlmError::Parse("Response message missing 'content' field".to_string()))?;

    // C# `content.GetString()` returns null for JSON null (and throws for a non-string
    // value, which lands in the same failure class here).
    content
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| LlmError::Parse("Response content is null".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Current-thread runtime with the IO and time drivers, so `block_on` can drive the
    /// `async fn call_api` (the crate's tokio features are `rt-multi-thread`, `time` and
    /// `sync`, and `macros` is not enabled, so no `#[tokio::test]`).
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    /// C# `ActionProfile.CleanOutput` system prompt, as the executor passes it in.
    const CLEAN_OUTPUT: &str = "Return only the processed result. Do not include any explanation, preamble, commentary, or extra text.";

    // Constructor validations — the C# ArgumentException messages, in source order.

    #[test]
    fn new_rejects_an_empty_api_key() {
        let error = OpenAiApiService::new("", "https://api.openai.com/v1", "gpt-4o-mini")
            .expect_err("empty key must fail");
        assert!(
            matches!(&error, LlmError::InvalidConfig(message) if message == "API key cannot be empty"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn new_rejects_a_whitespace_api_key() {
        let error = OpenAiApiService::new("   ", "https://api.openai.com/v1", "gpt-4o-mini")
            .expect_err("blank key must fail");
        assert!(
            matches!(&error, LlmError::InvalidConfig(message) if message == "API key cannot be empty"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn new_rejects_an_empty_base_url() {
        let error = OpenAiApiService::new("key", "\t", "gpt-4o-mini")
            .expect_err("empty base url must fail");
        assert!(
            matches!(&error, LlmError::InvalidConfig(message) if message == "Base URL cannot be empty"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn new_rejects_an_empty_model() {
        let error = OpenAiApiService::new("key", "https://api.openai.com/v1", "  ")
            .expect_err("empty model must fail");
        assert!(
            matches!(&error, LlmError::InvalidConfig(message) if message == "Model name cannot be empty"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn new_trims_the_base_url_slashes_and_the_key() {
        let service =
            OpenAiApiService::new("  sk-test  ", "https://example.test/v1///", "gpt-4o-mini")
                .expect("valid configuration");

        assert_eq!(
            service.endpoint_url, "https://example.test/v1/chat/completions",
            "only the trailing slashes of the base URL are dropped"
        );
        // The auth header is never traced; it is only compared here.
        assert_eq!(service.auth_header, "Bearer sk-test");
        // The model is not trimmed by the source.
        assert_eq!(service.model, "gpt-4o-mini");
    }

    #[test]
    fn new_accepts_a_base_url_without_a_trailing_slash() {
        let service = OpenAiApiService::new("sk", "https://example.test/v1", "m")
            .expect("valid configuration");
        assert_eq!(
            service.endpoint_url,
            "https://example.test/v1/chat/completions"
        );
    }

    #[test]
    fn debug_redacts_the_auth_header() {
        let service = OpenAiApiService::new("sk-super-secret", "https://example.test/v1", "m")
            .expect("valid");
        let rendered = format!("{service:?}");
        assert!(
            !rendered.contains("sk-super-secret"),
            "the API key must never appear in Debug output: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
    }

    // Request body shape.

    #[test]
    fn request_body_puts_the_system_message_first() {
        let body = ChatRequest::new("gpt-4o-mini", "hello {text}", None);
        let json = serde_json::to_string(&body).expect("serialize");
        assert_eq!(
            json,
            r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hello {text}"}],"max_tokens":2048}"#
        );
    }

    #[test]
    fn request_body_includes_the_clean_output_system_prompt() {
        let body = ChatRequest::new("gpt-4o-mini", "prompt", Some(CLEAN_OUTPUT));
        let json = serde_json::to_string(&body).expect("serialize");
        assert_eq!(
            json,
            r#"{"model":"gpt-4o-mini","messages":[{"role":"system","content":"Return only the processed result. Do not include any explanation, preamble, commentary, or extra text."},{"role":"user","content":"prompt"}],"max_tokens":2048}"#
        );
    }

    // Error-detail extraction.

    #[test]
    fn extract_error_message_prefers_the_error_message_field() {
        assert_eq!(
            extract_error_message(r#"{"error":{"message":"rate limited","code":429}}"#),
            "rate limited"
        );
    }

    #[test]
    fn extract_error_message_ignores_a_blank_error_message() {
        assert_eq!(
            extract_error_message(r#"{"error":{"message":"   "}}"#),
            r#"{"error":{"message":"   "}}"#
        );
    }

    #[test]
    fn extract_error_message_falls_back_to_the_raw_body_on_bad_json() {
        assert_eq!(extract_error_message("not json at all"), "not json at all");
    }

    #[test]
    fn extract_error_message_reports_no_details_for_a_blank_body() {
        assert_eq!(extract_error_message(""), "No error details provided");
        assert_eq!(extract_error_message("  \n "), "No error details provided");
    }

    #[test]
    fn extract_error_message_truncates_at_500_chars_with_three_dots() {
        let long = "a".repeat(700);
        assert_eq!(
            extract_error_message(&long),
            format!("{}...", "a".repeat(500))
        );
    }

    #[test]
    fn extract_error_message_keeps_a_body_of_exactly_500_chars() {
        let exact = "b".repeat(500);
        assert_eq!(extract_error_message(&exact), exact);
    }

    #[test]
    fn extract_error_message_counts_utf16_units_like_dotnet_string_length() {
        // U+1D11E is outside the BMP, so it is two UTF-16 units: 250 of them are exactly
        // 500 units, and 251 of them are 502.
        let musical_symbol = char::from_u32(0x1D11E).expect("valid scalar");
        let exact: String = std::iter::repeat_n(musical_symbol, 250).collect();
        assert_eq!(extract_error_message(&exact), exact);

        // The cut lands mid surrogate pair, exactly like `Substring(0, 500)` would.
        let long: String = std::iter::repeat_n(musical_symbol, 251).collect();
        assert_eq!(extract_error_message(&long), format!("{exact}..."));
    }

    // Response parsing — the source's exact failure strings.

    #[test]
    fn parse_response_reads_the_first_choice_content() {
        let json = r#"{"choices":[{"message":{"content":"result"}}],"usage":{}}"#;
        assert_eq!(parse_response(json).expect("content"), "result");
    }

    #[test]
    fn parse_response_reports_the_missing_choices_field() {
        let error = parse_response(r#"{"error":"nope"}"#).expect_err("missing choices");
        assert!(
            matches!(&error, LlmError::Parse(message) if message == "Response missing 'choices' field"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn parse_response_reports_an_empty_choices_array() {
        let error = parse_response(r#"{"choices":[]}"#).expect_err("empty choices");
        assert!(
            matches!(&error, LlmError::Parse(message) if message == "Response 'choices' array is empty"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn parse_response_reports_a_choices_value_that_is_not_an_array() {
        let error = parse_response(r#"{"choices":{"message":{"content":"x"}}}"#)
            .expect_err("choices must be an array");
        assert!(
            matches!(&error, LlmError::Parse(message) if message == "Response 'choices' array is empty"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn parse_response_reports_the_missing_message_field() {
        let error = parse_response(r#"{"choices":[{"finish_reason":"stop"}]}"#)
            .expect_err("choice without a message");
        assert!(
            matches!(&error, LlmError::Parse(message) if message == "Response choice missing 'message' field"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn parse_response_reports_the_missing_content_field() {
        let error = parse_response(r#"{"choices":[{"message":{"role":"assistant"}}]}"#)
            .expect_err("message without content");
        assert!(
            matches!(&error, LlmError::Parse(message) if message == "Response message missing 'content' field"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn parse_response_reports_a_null_content() {
        let error = parse_response(r#"{"choices":[{"message":{"content":null}}]}"#)
            .expect_err("null content");
        assert!(
            matches!(&error, LlmError::Parse(message) if message == "Response content is null"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn parse_response_reports_invalid_json() {
        let error = parse_response("<html>not json</html>").expect_err("not json");
        assert!(
            error
                .to_string()
                .starts_with("Failed to parse API response as JSON: "),
            "unexpected error: {error}"
        );
    }

    // End-to-end against a local HTTP server: the source's request shape and error
    // handling are observable wire behavior, so they are pinned with a mock rather than
    // with a test double.

    #[test]
    fn wire_mock_pins_the_request_shape_and_headers() {
        use wiremock::matchers::{body_partial_json, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        runtime().block_on(async {
            let server = MockServer::start().await;

            Mock::given(method("POST"))
                .and(path("/chat/completions"))
                .and(header("authorization", "Bearer sk-test"))
                .and(header("http-referer", HTTP_REFERER))
                .and(header("x-title", X_TITLE))
                .and(header("content-type", CONTENT_TYPE_WITH_CHARSET))
                .and(body_partial_json(serde_json::json!({
                    "model": "gpt-4o-mini",
                    "max_tokens": 2048u32,
                    "messages": [
                        {"role": "system", "content": CLEAN_OUTPUT},
                        {"role": "user", "content": "translate: hi"}
                    ]
                })))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "choices": [{"message": {"content": "mock response"}}]
                })))
                .expect(1)
                .mount(&server)
                .await;

            let service = OpenAiApiService::new("sk-test", &server.uri(), "gpt-4o-mini")
                .expect("valid configuration");

            let result = service
                .call_api("translate: hi", Some(CLEAN_OUTPUT))
                .await
                .expect("mock response");

            server.verify().await;
            assert_eq!(result, "mock response");
        });
    }

    #[test]
    fn wire_mock_maps_a_failure_status_to_api_status_with_the_error_message() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        runtime().block_on(async {
            let server = MockServer::start().await;

            Mock::given(method("POST"))
                .and(path("/chat/completions"))
                .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                    "error": {"message": "Rate limit reached for gpt-4o-mini", "code": "rate_limit"}
                })))
                .expect(1)
                .mount(&server)
                .await;

            let service = OpenAiApiService::new("sk", &server.uri(), "gpt-4o-mini")
                .expect("valid configuration");

            let error = service.call_api("hi", None).await.expect_err("must fail");

            server.verify().await;
            match error {
                LlmError::ApiStatus {
                    status,
                    reason,
                    detail,
                } => {
                    assert_eq!(status, 429);
                    // reqwest reports the RFC canonical reason phrase for the status.
                    assert!(!reason.is_empty(), "reason phrase should be present");
                    assert_eq!(detail, "Rate limit reached for gpt-4o-mini");
                }
                other => panic!("expected LlmError::ApiStatus, got {other:?}"),
            }
        });
    }

    #[test]
    fn wire_mock_truncates_a_long_non_json_error_body() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        runtime().block_on(async {
            let server = MockServer::start().await;

            Mock::given(method("POST"))
                .and(path("/chat/completions"))
                .respond_with(ResponseTemplate::new(500).set_body_string("z".repeat(900)))
                .expect(1)
                .mount(&server)
                .await;

            let service = OpenAiApiService::new("sk", &server.uri(), "gpt-4o-mini")
                .expect("valid configuration");

            let error = service.call_api("hi", None).await.expect_err("must fail");

            server.verify().await;
            match error {
                LlmError::ApiStatus { status, detail, .. } => {
                    assert_eq!(status, 500);
                    assert_eq!(detail.len(), 503);
                    assert!(detail.ends_with("..."));
                }
                other => panic!("expected LlmError::ApiStatus, got {other:?}"),
            }
        });
    }

    #[test]
    fn wire_mock_reports_a_body_that_is_not_json_as_a_parse_failure() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        runtime().block_on(async {
            let server = MockServer::start().await;

            Mock::given(method("POST"))
                .and(path("/chat/completions"))
                .respond_with(ResponseTemplate::new(200).set_body_string("definitely not json"))
                .expect(1)
                .mount(&server)
                .await;

            let service = OpenAiApiService::new("sk", &server.uri(), "gpt-4o-mini")
                .expect("valid configuration");

            let error = service.call_api("hi", None).await.expect_err("must fail");

            server.verify().await;
            assert!(
                error
                    .to_string()
                    .starts_with("Failed to parse API response as JSON: "),
                "unexpected error: {error}"
            );
        });
    }

    #[test]
    fn wire_mock_reports_a_success_body_without_a_usable_choices_shape() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        runtime().block_on(async {
            let server = MockServer::start().await;

            Mock::given(method("POST"))
                .and(path("/chat/completions"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "choices": [{"message": {"role": "assistant"}}]
                })))
                .expect(1)
                .mount(&server)
                .await;

            let service = OpenAiApiService::new("sk", &server.uri(), "gpt-4o-mini")
                .expect("valid configuration");

            let error = service.call_api("hi", None).await.expect_err("must fail");

            server.verify().await;
            assert!(
                matches!(&error, LlmError::Parse(message) if message == "Response message missing 'content' field"),
                "unexpected error: {error:?}"
            );
        });
    }

    #[test]
    fn wire_mock_maps_a_connection_failure_to_network() {
        runtime().block_on(async {
            // Port 1 on loopback is not listening, so reqwest's connect fails.
            let service = OpenAiApiService::new("sk", "http://127.0.0.1:1", "gpt-4o-mini")
                .expect("valid configuration");

            let error = service.call_api("hi", None).await.expect_err("must fail");

            assert!(
                matches!(error, LlmError::Network { .. }),
                "expected a network error, got {error:?}"
            );
        });
    }
}
