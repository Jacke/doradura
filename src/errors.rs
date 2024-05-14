use reqwest;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("HTTP request failed with status: {0}")]
    Http(reqwest::StatusCode),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
}
