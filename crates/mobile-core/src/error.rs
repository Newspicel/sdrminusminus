#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("Bad link: {reason}")]
    InvalidLink { reason: String },
    #[error("Wrong code")]
    WrongCode,
    #[error("Code expired")]
    CodeExpired,
    #[error("Server protocol {server}, app protocol {app}")]
    ProtocolMismatch { server: u32, app: u32 },
    #[error("Unreachable")]
    Unreachable { hosts: Vec<String> },
    #[error("Local network blocked")]
    LocalNetworkBlocked,
    #[error("Key changed")]
    KeyMismatch,
    #[error("Removed. Pair again")]
    Revoked,
    #[error("Vault error {status}")]
    Vault { status: i32 },
    #[error("Server error {status}: {message}")]
    Server { status: u16, message: String },
    #[error("Not connected")]
    NotConnected,
    #[error("No mission")]
    NoMission,
    #[error("{message}")]
    Refused { message: String },
    #[error("{message}")]
    Internal { message: String },
}

impl CoreError {
    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    pub(crate) fn stopped() -> Self {
        Self::internal("Core stopped")
    }
}

pub(crate) const UNEXPECTED_VAULT_STATUS: i32 = -1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum VaultError {
    #[error("Vault status {status}")]
    Os { status: i32 },
    #[error("Vault item corrupt")]
    Corrupt,
}

impl From<uniffi::UnexpectedUniFFICallbackError> for VaultError {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        tracing::warn!(reason = %error.reason, "vault callback failed");
        Self::Os {
            status: UNEXPECTED_VAULT_STATUS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unexpected_vault_exception_is_an_os_error() {
        let error = VaultError::from(uniffi::UnexpectedUniFFICallbackError {
            reason: "boom".to_owned(),
        });
        assert_eq!(
            error,
            VaultError::Os {
                status: UNEXPECTED_VAULT_STATUS
            }
        );
    }

    #[test]
    fn messages_stay_short() {
        assert_eq!(CoreError::Revoked.to_string(), "Removed. Pair again");
        assert_eq!(CoreError::stopped().to_string(), "Core stopped");
        assert_eq!(
            CoreError::ProtocolMismatch { server: 2, app: 1 }.to_string(),
            "Server protocol 2, app protocol 1"
        );
    }
}
