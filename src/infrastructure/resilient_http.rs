use anyhow::{Context, Result};
use reqwest::{Client, Response, Url};
use serde_json::Value;
use std::{net::SocketAddr, time::Duration};

pub async fn get(client: &Client, url: &str) -> Result<Response> {
    let parsed = Url::parse(url)?;
    match client.get(parsed.clone()).send().await {
        Ok(response) => Ok(response),
        Err(initial_error) => {
            let host = parsed.host_str().unwrap_or_default();
            if !spotify_host(host) {
                return Err(initial_error.into());
            }
            let ip = resolve_with_doh(host).await.with_context(|| {
                format!("request failed ({initial_error}); secure DNS fallback also failed")
            })?;
            let fallback = Client::builder()
                .timeout(Duration::from_secs(12))
                .user_agent("Ununknown/0.6.0")
                .resolve(host, SocketAddr::new(ip, 443))
                .build()?;
            fallback.get(parsed).send().await.with_context(|| {
                format!("request failed ({initial_error}); Spotify DNS fallback was unsuccessful")
            })
        }
    }
}

async fn resolve_with_doh(host: &str) -> Result<std::net::IpAddr> {
    let resolver = Client::builder()
        .timeout(Duration::from_secs(6))
        .resolve("cloudflare-dns.com", "1.1.1.1:443".parse::<SocketAddr>()?)
        .build()?;
    let raw = resolver
        .get("https://cloudflare-dns.com/dns-query")
        .header("Accept", "application/dns-json")
        .query(&[("name", host), ("type", "A")])
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    raw["Answer"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|answer| answer["type"].as_u64() == Some(1))
        .find_map(|answer| answer["data"].as_str()?.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("secure DNS returned no IPv4 address for {host}"))
}

fn spotify_host(host: &str) -> bool {
    matches!(host, "open.spotify.com" | "i.scdn.co")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_fallback_is_limited_to_spotify_hosts() {
        assert!(spotify_host("open.spotify.com"));
        assert!(spotify_host("i.scdn.co"));
        assert!(!spotify_host("localhost"));
        assert!(!spotify_host("example.com"));
    }
}
