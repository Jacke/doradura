#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_fetch_song_metadata_success() {
        let mock_server = MockServer::start().await;

        let response_body = r#"
        <html>
            <head>
                <title>Test Song</title>
                <meta property="og:artist" content="Test Artist">
            </head>
        </html>"#;
        let response = ResponseTemplate::new(200).set_body_string(response_body);

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(response)
            .mount(&mock_server)
            .await;

        let url = format!("{}/test", &mock_server.uri());
        let (title, artist) = fetch_song_metadata(&url).await.unwrap();

        assert_eq!(title, "Test Song");
        assert_eq!(artist, "Test Artist");
    }

    #[tokio::test]
    async fn test_fetch_song_metadata_no_artist() {
        let mock_server = MockServer::start().await;

        let response_body = r#"
        <html>
            <head>
                <title>Test Song</title>
            </head>
        </html>"#;
        let response = ResponseTemplate::new(200).set_body_string(response_body);

        Mock::given(method("GET"))
            .and(path("/test_no_artist"))
            .respond_with(response)
            .mount(&mock_server)
            .await;

        let url = format!("{}/test_no_artist", &mock_server.uri());
        let (title, artist) = fetch_song_metadata(&url).await.unwrap();

        assert_eq!(title, "Test Song");
        assert_eq!(artist, "");
    }

    #[tokio::test]
    async fn test_fetch_song_metadata_no_title() {
        let mock_server = MockServer::start().await;

        let response_body = r#"
        <html>
            <head>
                <meta property="og:artist" content="Test Artist">
            </head>
        </html>"#;
        let response = ResponseTemplate::new(200).set_body_string(response_body);

        Mock::given(method("GET"))
            .and(path("/test_no_title"))
            .respond_with(response)
            .mount(&mock_server)
            .await;

        let url = format!("{}/test_no_title", &mock_server.uri());
        let (title, artist) = fetch_song_metadata(&url).await.unwrap();

        assert_eq!(title, "");
        assert_eq!(artist, "Test Artist");
    }

    #[tokio::test]
    async fn test_fetch_song_metadata_error() {
        let mock_server = MockServer::start().await;

        let response = ResponseTemplate::new(404);

        Mock::given(method("GET"))
            .and(path("/test_error"))
            .respond_with(response)
            .mount(&mock_server)
            .await;

        let url = format!("{}/test_error", &mock_server.uri());
        let result = fetch_song_metadata(&url).await;

        assert!(result.is_err());
    }
}
