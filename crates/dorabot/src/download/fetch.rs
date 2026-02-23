use crate::core::error::AppError;
use crate::download::error::DownloadError;
use reqwest;
use select::document::Document;
use select::predicate::Name;

/// Fetches song metadata from a URL.
///
/// Downloads the HTML page and extracts the title and artist from meta tags.
///
/// # Arguments
///
/// * `url` - URL to fetch metadata from
///
/// # Returns
///
/// Returns a tuple `(title, artist)` or an `AppError`.
///
/// # Errors
///
/// Returns an error if:
/// - The HTTP request fails
/// - The HTTP response status is not successful
/// - The response body cannot be read
///
/// # Example
///
/// ```no_run
/// use doradura::download::fetch::fetch_song_metadata;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let (title, artist) = fetch_song_metadata("https://youtube.com/watch?v=...").await?;
/// println!("{} - {}", artist, title);
/// # Ok(())
/// # }
/// ```
pub async fn fetch_song_metadata(url: &str) -> Result<(String, String), AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::Download(DownloadError::Other(format!("HTTP client error: {}", e))))?;
    let resp = client.get(url).send().await?;

    if !resp.status().is_success() {
        return Err(AppError::HttpStatus(resp.status()));
    }

    let resp_text = resp.text().await?;
    let document = Document::from(resp_text.as_str());

    let title = document
        .find(Name("title"))
        .next()
        .map(|n| n.text())
        .unwrap_or_default();

    let artist = document
        .find(Name("meta"))
        .find(|n| n.attr("property").map(|v| v == "og:artist").unwrap_or(false))
        .and_then(|n| n.attr("content"))
        .unwrap_or_default()
        .to_string();

    Ok((title, artist))
}
