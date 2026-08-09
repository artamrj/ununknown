use anyhow::{Context, Result};
use reqwest::{Client, Response, Url};
use serde_json::Value;
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

pub async fn get(client: &Client, url: &str) -> Result<Response> {
    let parsed = Url::parse(url)?;
    match client.get(parsed.clone()).send().await {
        Ok(response) => Ok(response),
        Err(initial_error) => {
            let host = parsed.host_str().unwrap_or_default();
            if parsed.scheme() != "https"
                || !initial_error.is_connect()
                || !trusted_catalog_host(host)
            {
                return Err(initial_error.into());
            }
            let ip = resolve_with_doh(host).await.with_context(|| {
                format!("request failed ({initial_error}); secure DNS fallback also failed")
            })?;
            let fallback = Client::builder()
                .timeout(Duration::from_secs(12))
                .user_agent("Ununknown/0.6.0")
                .resolve(
                    host,
                    SocketAddr::new(ip, parsed.port_or_known_default().unwrap_or(443)),
                )
                .build()?;
            fallback.get(parsed).send().await.with_context(|| {
                format!(
                    "request failed ({initial_error}); secure catalog DNS fallback was unsuccessful"
                )
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
        .find_map(|answer| {
            let address = answer["data"].as_str()?.parse().ok()?;
            public_ip(address).then_some(address)
        })
        .ok_or_else(|| anyhow::anyhow!("secure DNS returned no public IPv4 address for {host}"))
}

fn trusted_catalog_host(host: &str) -> bool {
    matches!(
        host,
        "open.spotify.com" | "i.scdn.co" | "coverartarchive.org"
    ) || host.ends_with(".dzcdn.net")
        || host.ends_with(".mzstatic.com")
        || host.ends_with(".sndcdn.com")
        || host == "archive.org"
        || host.ends_with(".archive.org")
}

fn public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let [first, second, ..] = address.octets();
            !address.is_private()
                && !address.is_loopback()
                && !address.is_link_local()
                && !address.is_broadcast()
                && !address.is_documentation()
                && !address.is_unspecified()
                && !address.is_multicast()
                && !(first == 100 && (64..=127).contains(&second))
                && !(first == 198 && matches!(second, 18 | 19))
                && first < 224
        }
        IpAddr::V6(address) => {
            let segments = address.segments();
            let documentation = segments[0] == 0x2001 && segments[1] == 0x0db8;
            let mapped_public = address
                .to_ipv4_mapped()
                .is_none_or(|mapped| public_ip(IpAddr::V4(mapped)));
            mapped_public
                && !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_multicast()
                && !address.is_unique_local()
                && !address.is_unicast_link_local()
                && !documentation
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_fallback_is_limited_to_trusted_catalog_hosts() {
        assert!(trusted_catalog_host("open.spotify.com"));
        assert!(trusted_catalog_host("i.scdn.co"));
        assert!(trusted_catalog_host("cdn-images.dzcdn.net"));
        assert!(trusted_catalog_host("is1-ssl.mzstatic.com"));
        assert!(!trusted_catalog_host("dzcdn.net.example.com"));
        assert!(!trusted_catalog_host("localhost"));
        assert!(!trusted_catalog_host("example.com"));
    }

    #[test]
    fn secure_dns_fallback_rejects_non_public_addresses() {
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
        assert!(!public_ip("127.0.0.1".parse().unwrap()));
        assert!(!public_ip("192.168.1.2".parse().unwrap()));
        assert!(!public_ip("100.64.0.1".parse().unwrap()));
        assert!(!public_ip("::1".parse().unwrap()));
        assert!(!public_ip("fc00::1".parse().unwrap()));
        assert!(!public_ip("2001:db8::1".parse().unwrap()));
        assert!(!public_ip("::ffff:127.0.0.1".parse().unwrap()));
    }
}
