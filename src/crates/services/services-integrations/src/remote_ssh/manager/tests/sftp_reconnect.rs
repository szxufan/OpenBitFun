use super::super::*;

#[test]
fn sftp_requests_tolerate_a_slow_but_healthy_link() {
    let crate_default = russh_sftp::client::Config::default().request_timeout_secs;
    let configured = sftp_client_config().request_timeout_secs;
    assert_eq!(configured, SFTP_REQUEST_TIMEOUT_SECS);
    assert!(
        configured > crate_default,
        "the {crate_default}s dependency default is a throughput floor for one 256 KiB chunk"
    );
}

#[test]
fn only_transport_sftp_errors_trigger_a_reconnect() {
    let status = |status_code| {
        SftpError::Status(russh_sftp::protocol::Status {
            id: 1,
            status_code,
            error_message: String::new(),
            language_tag: String::new(),
        })
    };

    assert!(sftp_error_may_be_stale_transport(&status(
        SftpStatusCode::ConnectionLost
    )));
    assert!(sftp_error_may_be_stale_transport(&SftpError::Timeout));
    assert!(!sftp_error_may_be_stale_transport(&status(
        SftpStatusCode::NoSuchFile
    )));
    assert!(!sftp_error_may_be_stale_transport(&status(
        SftpStatusCode::PermissionDenied
    )));
}
