//! MTProto-specific error types

use thiserror::Error;

/// Errors that can occur during MTProto operations
#[derive(Error, Debug)]
pub enum MtProtoError {
    /// Failed to decode Bot API file_id
    #[error("Failed to decode file_id: {0}")]
    FileIdDecode(String),

    /// Unsupported file type for download
    #[error("Unsupported file type: {0:?}")]
    UnsupportedFileType(super::file_id::FileType),

    /// Message not found in chat
    #[error("Message not found")]
    MessageNotFound,

    /// Message has no media attachment
    #[error("No media in message")]
    NoMediaInMessage,

    /// CDN redirect not supported (would require additional implementation)
    #[error("CDN redirect not supported")]
    CdnRedirectNotSupported,

    /// Grammers client invocation error
    #[error("MTProto client error: {0}")]
    Invocation(#[from] grammers_mtsender::InvocationError),

    /// Session-related errors
    #[error("Session error: {0}")]
    Session(String),

    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Base64 decoding errors
    #[error("Base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),

    /// Client not authorized
    #[error("Client not authorized")]
    NotAuthorized,

    /// Sign-in failed
    #[error("Sign-in failed: {0}")]
    SignIn(String),

    /// File reference expired (need to refresh)
    #[error("File reference expired")]
    FileReferenceExpired,

    /// DC migration required but failed
    #[error("DC migration failed: {0}")]
    DcMigration(String),
}

impl From<MtProtoError> for crate::core::error::AppError {
    fn from(err: MtProtoError) -> Self {
        crate::core::error::AppError::Download(format!("MTProto error: {}", err))
    }
}
