use reqwest;
use select::document::Document;
use select::predicate::Name;
use crate::errors::FetchError;

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
