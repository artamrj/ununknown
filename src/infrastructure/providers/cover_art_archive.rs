use anyhow::Result;
use reqwest::Client;

pub async fn fetch(client: &Client, url: &str) -> Result<Vec<u8>> {
    Ok(client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec())
}
