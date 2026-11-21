use super::super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn spawn_single_review_response(response: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock provider should bind");
    let address = listener.local_addr().expect("mock provider address");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("mock provider should accept");
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        stream
            .write_all(&response)
            .expect("mock provider response should write");
    });
    format!("http://{address}")
}

fn spawn_review_request_probe() -> (String, mpsc::Receiver<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock provider should bind");
    listener
        .set_nonblocking(true)
        .expect("mock provider should allow non-blocking accept");
    let address = listener.local_addr().expect("mock provider address");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_millis(300);
        loop {
            match listener.accept() {
                Ok(_) => {
                    let _ = sender.send(true);
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        let _ = sender.send(false);
                        return;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("mock provider probe should accept: {error}"),
            }
        }
    });
    (format!("http://{address}"), receiver)
}

fn gitlab_trace_context(api_base_url: String) -> ProviderContext {
    let mut tokens = ReviewPlatformAuthTokens::default();
    tokens.tokens.insert(
        token_key(ReviewPlatformKind::Gitlab, "gitlab.com")
            .expect("GitLab token key should normalize"),
        "never-expose-trace-token".to_string(),
    );
    let mut context = provider_context_for_identity(
        ReviewPlatformKind::Gitlab,
        "gitlab.com",
        "example/repo",
        &tokens,
    )
    .expect("GitLab trace context should be valid");
    context.api_base_url = api_base_url;
    context
}

#[test]
fn summarize_ci_items_counts_provider_outcomes() {
    let items = vec![
        ReviewPlatformCiItem {
            id: "build".to_string(),
            name: "Build".to_string(),
            status: "completed".to_string(),
            conclusion: Some("success".to_string()),
            detail: None,
            stage: Some("build".to_string()),
            web_url: None,
            log: None,
            log_truncated: false,
            started_at: None,
            finished_at: None,
        },
        ReviewPlatformCiItem {
            id: "test".to_string(),
            name: "Test".to_string(),
            status: "failed".to_string(),
            conclusion: None,
            detail: None,
            stage: Some("test".to_string()),
            web_url: None,
            log: None,
            log_truncated: false,
            started_at: None,
            finished_at: None,
        },
        ReviewPlatformCiItem {
            id: "deploy".to_string(),
            name: "Deploy".to_string(),
            status: "running".to_string(),
            conclusion: None,
            detail: None,
            stage: Some("deploy".to_string()),
            web_url: None,
            log: None,
            log_truncated: false,
            started_at: None,
            finished_at: None,
        },
    ];

    let checks = summarize_ci_items(&items);

    assert_eq!(checks.total, 3);
    assert_eq!(checks.passed, 1);
    assert_eq!(checks.failed, 1);
    assert_eq!(checks.pending, 1);
}

#[test]
fn ci_log_value_extracts_error_excerpt_only() {
    let text = [
        "running setup",
        "downloading dependencies",
        "cargo test failed with exit code 101",
        "thread 'main' panicked at src/lib.rs:4",
        "uploading artifacts",
    ]
    .join("\n");

    let (log, truncated) = ci_log_value(text);

    let log = log.expect("log should be present");
    assert!(!truncated);
    assert!(log.contains("cargo test failed"));
    assert!(log.contains("panicked at src/lib.rs"));
}

#[test]
fn ci_log_value_reports_when_no_error_lines_match() {
    let (log, truncated) = ci_log_value("all checks passed".to_string());

    assert!(!truncated);
    assert!(log.is_none());
}

#[test]
fn ci_trace_transport_truncation_is_preserved_as_partial_log_state() {
    let (log, truncated) = ci_log_value_from_response(ReviewTextResponse {
        text: "error: failed before provider trace budget".to_string(),
        truncated: true,
    });

    assert_eq!(
        log.as_deref(),
        Some("error: failed before provider trace budget")
    );
    assert!(truncated);
}

#[tokio::test]
async fn gitlab_trace_http_failures_propagate_without_provider_body() {
    for status in [401u16, 500u16] {
        let body = "provider-secret-error-body";
        let api_base_url = spawn_single_review_response(
                format!(
                    "HTTP/1.1 {status} Failure\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .into_bytes(),
            );
        let context = gitlab_trace_context(api_base_url);

        let result = gitlab_pull_request_ci_log(&context, "7", "123", "job").await;

        assert!(matches!(
            result,
            Err(ReviewPlatformError::Http {
                status: actual,
                ref message,
            }) if actual == status && message.is_empty()
        ));
    }
}

#[tokio::test]
async fn gitlab_trace_rejects_blank_job_id_without_provider_call() {
    let (api_base_url, provider_called) = spawn_review_request_probe();
    let context = gitlab_trace_context(api_base_url);

    let result = gitlab_pull_request_ci_log(&context, "7", "   ", "job").await;

    assert!(matches!(
        result,
        Err(ReviewPlatformError::Api(ref message))
            if message == "GitLab job id is required"
    ));
    assert!(!provider_called
        .recv_timeout(Duration::from_secs(1))
        .expect("provider request probe should finish"));
}

#[tokio::test]
async fn gitlab_trace_successful_empty_body_remains_empty_evidence() {
    let api_base_url = spawn_single_review_response(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    );
    let context = gitlab_trace_context(api_base_url);

    let result = gitlab_pull_request_ci_log(&context, "7", "123", "job")
        .await
        .expect("successful empty trace should remain available");

    assert!(result.log.is_none());
    assert!(!result.truncated);
    assert_eq!(
        result.message.as_deref(),
        Some("No error lines were detected in the job trace.")
    );
}

#[tokio::test]
async fn gitlab_trace_interrupted_chunked_response_propagates_network_failure() {
    let api_base_url = spawn_single_review_response(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n20\r\nerror"
            .to_vec(),
    );
    let context = gitlab_trace_context(api_base_url);

    let result = gitlab_pull_request_ci_log(&context, "7", "123", "job").await;

    assert!(matches!(result, Err(ReviewPlatformError::Network(_))));
}
