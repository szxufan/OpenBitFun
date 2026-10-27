//! SMS verification-code delivery for relay-local accounts.
//!
//! Codes are delivered through Aliyun SMS (`dysmsapi.aliyuncs.com`, action
//! `SendSms`). Credentials, signature, and template come from the operator
//! environment; nothing here is client-selectable. When the environment is
//! incomplete, sending fails closed with an explicit "unavailable" state —
//! password login keeps working, code login does not.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Duration;

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub const ENV_ACCESS_KEY_ID: &str = "RELAY_SMS_ALIYUN_ACCESS_KEY_ID";
pub const ENV_ACCESS_KEY_SECRET: &str = "RELAY_SMS_ALIYUN_ACCESS_KEY_SECRET";
pub const ENV_SIGN_NAME: &str = "RELAY_SMS_ALIYUN_SIGN_NAME";
pub const ENV_TEMPLATE_CODE: &str = "RELAY_SMS_ALIYUN_TEMPLATE_CODE";
/// Local HMAC key for digesting verification codes at rest. Independent from
/// the Aliyun secret so rotating provider credentials does not invalidate
/// outstanding codes, and vice versa.
pub const ENV_CODE_SECRET: &str = "RELAY_SMS_CODE_SECRET";
pub const ENV_ENDPOINT: &str = "RELAY_SMS_ENDPOINT";

pub const DEFAULT_ENDPOINT: &str = "https://dysmsapi.aliyuncs.com";
const SMS_API_VERSION: &str = "2017-05-25";
const SMS_ACTION_SEND: &str = "SendSms";

/// Operator-provided Aliyun SMS settings.
#[derive(Debug, Clone)]
pub struct SmsConfig {
    pub access_key_id: String,
    pub access_key_secret: String,
    pub sign_name: String,
    pub template_code: String,
    /// Test deployments point this at a local mock server.
    pub endpoint: String,
}

impl SmsConfig {
    /// Read the Aliyun settings. Returns `None` (with a one-time warning) when
    /// any required variable is unset; the auth routes then answer 503 instead
    /// of pretending codes were sent.
    pub fn from_env() -> Option<Self> {
        let read = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let config = Self {
            access_key_id: read(ENV_ACCESS_KEY_ID)?,
            access_key_secret: read(ENV_ACCESS_KEY_SECRET)?,
            sign_name: read(ENV_SIGN_NAME)?,
            template_code: read(ENV_TEMPLATE_CODE)?,
            endpoint: read(ENV_ENDPOINT).unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
        };
        if config.endpoint != DEFAULT_ENDPOINT {
            tracing::warn!(
                "Aliyun SMS endpoint overridden for testing: {}",
                config.endpoint
            );
        }
        Some(config)
    }
}

/// Delivery backend. Object-safe so tests can intercept sends.
pub trait SmsTransport: Send + Sync {
    fn send<'a>(
        &'a self,
        phone: &'a str,
        code: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
}

/// Production transport: signed Aliyun SendSms calls.
pub struct AliyunSmsTransport {
    config: SmsConfig,
    client: reqwest::Client,
}

impl AliyunSmsTransport {
    pub fn new(config: SmsConfig) -> Self {
        // Standalone relay does not link the workspace service facade. Select
        // ring for this client without installing a second process provider,
        // mirroring the identity verifier's TLS setup.
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("safe protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
        let client = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("sms http client");
        Self { config, client }
    }
}

impl SmsTransport for AliyunSmsTransport {
    fn send<'a>(
        &'a self,
        phone: &'a str,
        code: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let Some(host) = endpoint_host(&self.config.endpoint) else {
                return Err("invalid sms endpoint".to_string());
            };
            let template_param = serde_json::json!({ "code": code }).to_string();
            let mut params: Vec<(&str, String)> = vec![
                ("PhoneNumbers", phone.to_string()),
                ("SignName", self.config.sign_name.clone()),
                ("TemplateCode", self.config.template_code.clone()),
                ("TemplateParam", template_param),
            ];
            params.sort_by(|a, b| a.0.cmp(b.0));
            let canonical_query = canonical_query_string(&params);
            let url = format!("{}?{}", self.config.endpoint, canonical_query);
            let signed = signed_request(
                &self.config.access_key_id,
                &self.config.access_key_secret,
                "POST",
                &host,
                &canonical_query,
                b"",
            );
            let response = self
                .client
                .post(&url)
                .header("host", &host)
                .header("x-acs-action", SMS_ACTION_SEND)
                .header("x-acs-version", SMS_API_VERSION)
                .header("x-acs-date", &signed.acs_date)
                .header("x-acs-signature-nonce", &signed.nonce)
                .header("x-acs-content-sha256", &signed.content_sha256)
                .header("authorization", &signed.authorization)
                .send()
                .await
                .map_err(|e| format!("sms transport error: {e}"))?;
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| format!("sms response read error: {e}"))?;
            let payload = serde_json::from_str::<SmsSendResponse>(&body)
                .map_err(|e| format!("sms response parse error: {e}"))?;
            if status.is_success() && payload.code.as_deref() == Some("OK") {
                return Ok(());
            }
            let code = payload.code.unwrap_or_else(|| "Unknown".to_string());
            Err(format!(
                "sms delivery rejected ({status}): {code}: {}",
                payload.message.unwrap_or_default()
            ))
        })
    }
}

/// Aliyun answers with PascalCase keys (`Code`, `Message`, `RequestId`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SmsSendResponse {
    code: Option<String>,
    message: Option<String>,
}

/// Local HMAC secret for code digests, resolved once per process.
pub fn code_secret() -> Option<&'static String> {
    static SECRET: OnceLock<Option<String>> = OnceLock::new();
    SECRET
        .get_or_init(|| {
            std::env::var(ENV_CODE_SECRET)
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .as_ref()
}

/// Digest a verification code for storage. The phone is mixed in so a leaked
/// table cannot be replayed against a different number.
pub fn code_digest(secret: &str, phone: &str, code: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key");
    mac.update(format!("{phone}:{code}").as_bytes());
    hex(&mac.finalize().into_bytes())
}

/// Random six-digit verification code. Leading zeros are preserved.
pub fn generate_code() -> String {
    use rand::Rng;
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32))
}

/// RFC 3986 percent-encoding (unreserved set only, uppercase hex), as the
/// Aliyun V3 signature requires.
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Encoded `k=v&k=v` query string. Callers pass pairs sorted by key.
pub fn canonical_query_string(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Host[:port] of an endpoint URL for the signed `host` header.
pub fn endpoint_host(endpoint: &str) -> Option<String> {
    let after_scheme = endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))?;
    let authority = after_scheme.split(['/', '?']).next()?;
    if authority.is_empty() {
        None
    } else {
        Some(authority.to_string())
    }
}

pub struct SignedRequest {
    pub acs_date: String,
    pub nonce: String,
    pub content_sha256: String,
    pub authorization: String,
}

/// Build ACS3-HMAC-SHA256 signed request metadata for one call.
///
/// Signature V3 signs the canonical request (method, URI, query, headers,
/// payload hash) with the AccessKeySecret; business parameters travel in the
/// query string and the body stays empty. `x-acs-action`/`x-acs-version` name
/// the API, so they are fixed here alongside the signed header set.
pub fn signed_request(
    access_key_id: &str,
    access_key_secret: &str,
    method: &str,
    host: &str,
    canonical_query: &str,
    body: &[u8],
) -> SignedRequest {
    sign(
        access_key_id,
        access_key_secret,
        method,
        "/",
        host,
        canonical_query,
        body,
        SMS_ACTION_SEND,
        SMS_API_VERSION,
        &chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string(),
        &uuid::Uuid::new_v4().to_string(),
    )
}

/// Test hook with injectable date/nonce for golden vectors.
#[cfg(test)]
fn signed_request_at(
    access_key_id: &str,
    access_key_secret: &str,
    method: &str,
    canonical_uri: &str,
    host: &str,
    canonical_query: &str,
    body: &[u8],
    action: &str,
    version: &str,
    acs_date: &str,
    nonce: &str,
) -> SignedRequest {
    sign(
        access_key_id,
        access_key_secret,
        method,
        canonical_uri,
        host,
        canonical_query,
        body,
        action,
        version,
        acs_date,
        nonce,
    )
}

fn sign(
    access_key_id: &str,
    access_key_secret: &str,
    method: &str,
    canonical_uri: &str,
    host: &str,
    canonical_query: &str,
    body: &[u8],
    action: &str,
    version: &str,
    acs_date: &str,
    nonce: &str,
) -> SignedRequest {
    let content_sha256 = hex(&Sha256::digest(body));
    let mut headers: Vec<(&str, String)> = vec![
        ("host", host.to_string()),
        ("x-acs-action", action.to_string()),
        ("x-acs-content-sha256", content_sha256.clone()),
        ("x-acs-date", acs_date.to_string()),
        ("x-acs-signature-nonce", nonce.to_string()),
        ("x-acs-version", version.to_string()),
    ];
    headers.sort_by(|a, b| a.0.cmp(b.0));
    let canonical_headers = headers
        .iter()
        .map(|(k, v)| format!("{k}:{v}\n"))
        .collect::<String>();
    let signed_headers = headers
        .iter()
        .map(|(k, _)| *k)
        .collect::<Vec<_>>()
        .join(";");
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{content_sha256}"
    );
    let string_to_sign = format!(
        "ACS3-HMAC-SHA256\n{}",
        hex(&Sha256::digest(canonical_request.as_bytes()))
    );
    let mut mac =
        HmacSha256::new_from_slice(access_key_secret.as_bytes()).expect("HMAC accepts any key");
    mac.update(string_to_sign.as_bytes());
    let signature = hex(&mac.finalize().into_bytes());
    SignedRequest {
        acs_date: acs_date.to_string(),
        nonce: nonce.to_string(),
        content_sha256,
        authorization: format!(
            "ACS3-HMAC-SHA256 Credential={access_key_id},SignedHeaders={signed_headers},Signature={signature}"
        ),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn test_config(endpoint: String) -> SmsConfig {
        SmsConfig {
            access_key_id: "test-ak".to_string(),
            access_key_secret: "test-sk".to_string(),
            sign_name: "OpenBitFun".to_string(),
            template_code: "SMS_123456".to_string(),
            endpoint,
        }
    }

    #[test]
    fn percent_encoding_matches_aliyun_v3_rules() {
        assert_eq!(percent_encode("abcXYZ09-_.~"), "abcXYZ09-_.~");
        assert_eq!(percent_encode("a b/c+d"), "a%20b%2Fc%2Bd");
        assert_eq!(
            percent_encode("{\"code\":\"123456\"}"),
            "%7B%22code%22%3A%22123456%22%7D"
        );
    }

    #[test]
    fn canonical_query_encodes_sorted_pairs() {
        let params = [
            ("PhoneNumbers", "13800138000".to_string()),
            ("TemplateParam", "{\"code\":\"1 2\"}".to_string()),
        ];
        assert_eq!(
            canonical_query_string(&params),
            "PhoneNumbers=13800138000&TemplateParam=%7B%22code%22%3A%221%202%22%7D"
        );
    }

    #[test]
    fn endpoint_host_extracts_authority_with_port() {
        assert_eq!(
            endpoint_host("https://dysmsapi.aliyuncs.com").as_deref(),
            Some("dysmsapi.aliyuncs.com")
        );
        assert_eq!(
            endpoint_host("http://127.0.0.1:9911").as_deref(),
            Some("127.0.0.1:9911")
        );
        assert_eq!(endpoint_host("not-a-url/"), None);
    }

    /// The signature must follow the documented V3 composition; this golden
    /// vector pins the canonical request and HMAC derivation exactly.
    #[test]
    fn signature_follows_the_v3_composition() {
        let signed = signed_request_at(
            "test-ak",
            "test-sk",
            "POST",
            "/",
            "dysmsapi.aliyuncs.com",
            "PhoneNumbers=13800138000",
            b"",
            "SendSms",
            "2017-05-25",
            "20260101T000000Z",
            "fixed-nonce",
        );
        let empty_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(signed.content_sha256, empty_hash);
        // Canonical headers each end with a newline, so a blank line separates
        // the header block from the signed-header list (SigV4-style layout).
        let canonical_request = [
            "POST\n/\nPhoneNumbers=13800138000\n",
            "host:dysmsapi.aliyuncs.com\n",
            "x-acs-action:SendSms\n",
            &format!("x-acs-content-sha256:{empty_hash}\n"),
            "x-acs-date:20260101T000000Z\n",
            "x-acs-signature-nonce:fixed-nonce\n",
            "x-acs-version:2017-05-25\n",
            "\n",
            "host;x-acs-action;x-acs-content-sha256;x-acs-date;x-acs-signature-nonce;x-acs-version\n",
            empty_hash,
        ]
        .concat();
        let expected_string_to_sign = format!(
            "ACS3-HMAC-SHA256\n{}",
            hex(&Sha256::digest(canonical_request.as_bytes()))
        );
        let mut mac = HmacSha256::new_from_slice(b"test-sk").unwrap();
        mac.update(expected_string_to_sign.as_bytes());
        let expected_signature = hex(&mac.finalize().into_bytes());
        assert_eq!(
            signed.authorization,
            format!(
                "ACS3-HMAC-SHA256 Credential=test-ak,\
                 SignedHeaders=host;x-acs-action;x-acs-content-sha256;x-acs-date;x-acs-signature-nonce;x-acs-version,\
                 Signature={expected_signature}"
            )
        );
    }

    #[tokio::test]
    async fn aliyun_transport_signs_and_sends_the_expected_request() {
        use tower::ServiceExt;

        #[derive(Clone)]
        struct Capture(Arc<std::sync::Mutex<Vec<(String, String)>>>);
        let received = Capture(Arc::new(std::sync::Mutex::new(Vec::new())));
        let captured = received.clone();
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(
                move |uri: axum::http::Uri, headers: axum::http::HeaderMap| {
                    let captured = captured.clone();
                    async move {
                        let authorization = headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_string();
                        let action = headers
                            .get("x-acs-action")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_string();
                        captured.0.lock().unwrap().push((
                            uri.query().unwrap_or("").to_string(),
                            format!("{authorization}|{action}"),
                        ));
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(
                                serde_json::json!({"RequestId":"r","Code":"OK","BizId":"b"}),
                            ),
                        )
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let transport = AliyunSmsTransport::new(test_config(format!("http://{addr}")));
        transport.send("13800138000", "123456").await.unwrap();

        let seen = received.0.lock().unwrap();
        let (query, headers) = &seen[0];
        assert!(query.contains("PhoneNumbers=13800138000"));
        assert!(query.contains("SignName=OpenBitFun"));
        assert!(query.contains("TemplateCode=SMS_123456"));
        assert!(query.contains("TemplateParam=%7B%22code%22%3A%22123456%22%7D"));
        // Business parameters sorted; the RPC action travels as a header.
        assert!(!query.contains("Action="));
        assert!(headers.starts_with("ACS3-HMAC-SHA256 Credential=test-ak,"));
        assert!(headers.contains("|SendSms"));
        task.abort();
    }

    #[tokio::test]
    async fn aliyun_transport_surfaces_provider_rejections() {
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "RequestId":"r","Code":"isv.BUSINESS_LIMIT_CONTROL","Message":"limit"
                    })),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let transport = AliyunSmsTransport::new(test_config(format!("http://{addr}")));
        let error = transport.send("13800138000", "123456").await.unwrap_err();
        assert!(error.contains("isv.BUSINESS_LIMIT_CONTROL"));
        task.abort();
    }

    #[test]
    fn code_digest_is_deterministic_and_phone_bound() {
        let a = code_digest("secret", "13800138000", "123456");
        let b = code_digest("secret", "13800138000", "123456");
        let other_phone = code_digest("secret", "13800138001", "123456");
        assert_eq!(a, b);
        assert_ne!(a, other_phone);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn generated_codes_are_six_digits() {
        for _ in 0..50 {
            let code = generate_code();
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|b| b.is_ascii_digit()));
        }
    }
}
