//! Account App Server wire schemas.

#[cfg(feature = "rpc")]
use agent_client_protocol::{JsonRpcRequest, JsonRpcResponse};
use serde::{Deserialize, Serialize};

pub use openbitfun_product_domains::account::{AccountDevice, AccountInfo};

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc", derive(JsonRpcRequest))]
#[cfg_attr(feature = "rpc", request(method = "account/snapshot", response = AccountSnapshotResponse))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountSnapshotRequest {
    pub workspace_path: String,
}

impl std::fmt::Debug for AccountSnapshotRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccountSnapshotRequest")
            .field("workspace_path", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc", derive(JsonRpcResponse))]
#[serde(rename_all = "camelCase")]
pub struct AccountSnapshotResponse {
    pub logged_in: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<AccountInfo>,
    #[serde(default)]
    pub devices: Vec<AccountDevice>,
}

/// Request an SMS verification code for relay-local sign-in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc", derive(JsonRpcRequest))]
#[cfg_attr(feature = "rpc", request(method = "account/smsSend", response = AccountSmsSendResponse))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountSmsSendRequest {
    pub phone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc", derive(JsonRpcResponse))]
#[serde(rename_all = "camelCase")]
pub struct AccountSmsSendResponse {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc", derive(JsonRpcRequest))]
#[cfg_attr(feature = "rpc", request(method = "account/login", response = AccountLoginResponse))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountLoginRequest {
    pub operation_id: String,
    /// Relay-local credentials: the phone plus exactly one of code/password.
    /// Older clients that omit them get an explicit error instead of a
    /// credential-less login attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc", derive(JsonRpcResponse))]
#[serde(rename_all = "camelCase")]
pub struct AccountLoginResponse {
    pub user_id: String,
    pub relay_url: String,
    pub status_message: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc", derive(JsonRpcRequest))]
#[cfg_attr(feature = "rpc", request(method = "account/logout", response = AccountSnapshotResponse))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountLogoutRequest {
    pub operation_id: String,
    pub workspace_path: String,
}

impl std::fmt::Debug for AccountLogoutRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccountLogoutRequest")
            .field("operation_id", &self.operation_id)
            .field("workspace_path", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_takes_optional_local_credentials() {
        // Older clients omit the credential fields entirely.
        assert!(serde_json::from_value::<AccountLoginRequest>(
            serde_json::json!({"operationId":"op"})
        )
        .is_ok());
        assert!(serde_json::from_value::<AccountLoginRequest>(
            serde_json::json!({"operationId":"op","phone":"13800138000","code":"123456"})
        )
        .is_ok());
        assert!(serde_json::from_value::<AccountLoginRequest>(
            serde_json::json!({"operationId":"op","phone":"13800138000","password":"secret"})
        )
        .is_ok());
        // Relay configuration never travels through this request.
        assert!(serde_json::from_value::<AccountLoginRequest>(
            serde_json::json!({"operationId":"op","relayUrl":"https://relay.example"})
        )
        .is_err());
    }

    #[test]
    fn account_methods_follow_lower_camel_method_contract() {
        for method in [
            "account/snapshot",
            "account/login",
            "account/logout",
            "account/smsSend",
        ] {
            assert!(crate::method::is_valid_method_name(method));
        }
    }
}
