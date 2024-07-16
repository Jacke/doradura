use reqwest;
use select::document::Document;
use select::predicate::Name;

// src/fetch.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("HTTP request failed with status: {0}")]
    Http(reqwest::StatusCode),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
}


/*
pub async fn fetch_url_to_file(url: &Url, file: &mut File) -> Result<(), Box<dyn Error + Send + Sync>> {
    let response = reqwest::get(url.as_str()).await?;
    let bytes = response.bytes().await?;
    file.write_all(&bytes).await?;
    Ok(())
}*/

pub async fn fetch_song_metadata(url: &str) -> Result<(String, String), FetchError> {
    let resp = reqwest::get(url).await?;

    if !resp.status().is_success() {
        return Err(FetchError::Http(resp.status()));
    }

    let resp_text = resp.text().await?;
    let document = Document::from(resp_text.as_str());

    let title = document.find(Name("title")).next().map(|n| n.text()).unwrap_or_default();

    let artist = document.find(Name("meta"))
        .filter(|n| n.attr("property").map(|v| v == "og:artist").unwrap_or(false))
        .next()
        .and_then(|n| n.attr("content"))
        .unwrap_or_default()
        .to_string();

    Ok((title, artist))
}
