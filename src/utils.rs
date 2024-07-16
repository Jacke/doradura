// src/utils.rs
// use std::error::Error;
// use tokio::fs::File;
// use tokio::io::AsyncWriteExt;
// use url::Url;
// use crate::fetch::fetch_url_to_file;

pub fn escape_filename(filename: &str) -> String {
    filename.replace("/", "_")
}

/*
pub async fn download_file(destination: &str, url: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let url = Url::parse(url)?;
    let mut file = File::create(destination).await?;
    fetch_url_to_file(&url, &mut file).await?;
    file.flush().await?;
    Ok(())
}
 */