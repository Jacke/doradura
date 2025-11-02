use reqwest;
use select::document::Document;
use select::predicate::Name;
use crate::error::AppError;

/// Получает метаданные песни из URL.
/// 
/// Загружает HTML-страницу и извлекает заголовок и исполнителя из мета-тегов.
/// 
/// # Arguments
/// 
/// * `url` - URL для получения метаданных
/// 
/// # Returns
/// 
/// Возвращает кортеж `(title, artist)` или ошибку `AppError`.
/// 
/// # Errors
/// 
/// Возвращает ошибку если:
/// - Не удалось выполнить HTTP-запрос
/// - HTTP-статус ответа не успешный
/// - Не удалось прочитать тело ответа
/// 
/// # Example
/// 
/// ```no_run
/// use doradura::fetch::fetch_song_metadata;
/// 
/// # async fn example() {
/// let (title, artist) = fetch_song_metadata("https://youtube.com/watch?v=...").await?;
/// println!("{} - {}", artist, title);
/// # }
/// ```
pub async fn fetch_song_metadata(url: &str) -> Result<(String, String), AppError> {
    let resp = reqwest::get(url).await?;

    if !resp.status().is_success() {
        return Err(AppError::HttpStatus(resp.status()));
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
