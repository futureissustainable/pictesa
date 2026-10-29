//! Guards for server-side fetches of user-supplied URLs (imports, custom provider endpoints).
//! Blocks private, loopback, link-local, carrier-grade NAT and other non-public addresses,
//! pins the checked address for the actual connection, and follows redirects only after
//! re-checking each hop.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use url::Url;

pub fn v4_is_public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || o[0] == 0
        || (o[0] == 100 && (64..=127).contains(&o[1]))      // carrier-grade NAT (also Tailscale)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)           // IETF protocol assignments
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))       // benchmarking
        || o[0] >= 240)                                      // reserved
}

pub fn ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_is_public(v4),
        IpAddr::V6(v6) => v6_is_public(v6),
    }
}

fn v6_is_public(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return v4_is_public(v4);
    }
    let s = ip.segments();
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00        // unique local
        || (s[0] & 0xffc0) == 0xfe80        // link local
        || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
        || (s[0] == 0x64 && s[1] == 0xff9b)) // NAT64 can reach private v4
}

/// Resolves the URL's host and returns one public address to connect to, or why it is refused.
pub async fn public_address(url: &Url) -> Result<SocketAddr, String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http and https URLs are allowed".into());
    }
    let host = url.host_str().ok_or("the URL has no host")?;
    let port = url.port_or_known_default().ok_or("the URL has no port")?;
    // One answer for names that do not resolve and for private addresses, so the reply never tells
    // which internal names exist.
    const REFUSED: &str = "the address is not a public server this one can reach";
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.trim_start_matches('[').trim_end_matches(']'), port))
        .await
        .map_err(|_| REFUSED.to_string())?
        .collect();
    // Every answer must be public, or DNS could alternate between a public and a private address.
    if addrs.is_empty() || addrs.iter().any(|a| !ip_is_public(a.ip())) {
        tracing::debug!("refused address {host}: {} answers, private: {}", addrs.len(), addrs.iter().any(|a| !ip_is_public(a.ip())));
        return Err(REFUSED.into());
    }
    Ok(addrs[0])
}

/// GETs a public URL with the checked address pinned, following up to five checked redirects.
pub async fn fetch_public(mut url: Url, user_agent: &str) -> Result<reqwest::Response, String> {
    for _ in 0..6 {
        let addr = public_address(&url).await?;
        let host = url.host_str().unwrap_or_default().to_string();
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&host, addr)
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client.get(url.clone()).send().await.map_err(|_| "the download failed".to_string())?;
        if resp.status().is_redirection() {
            let next = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|loc| url.join(loc).ok())
                .ok_or("the server sent a broken redirect")?;
            url = next;
            continue;
        }
        return Ok(resp);
    }
    Err("too many redirects".into())
}

/// DNS resolver that only returns public addresses. Every connection of a client built with it,
/// including each redirect hop, resolves through this check, and the checked address is the one used.
struct PublicResolver;

impl reqwest::dns::Resolve for PublicResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.filter(|a| ip_is_public(a.ip())).collect();
            if addrs.is_empty() {
                return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!("{host} does not resolve to a public address")));
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// A client that never reaches private addresses: names resolve through [`PublicResolver`] and
/// redirects to literal private IPs are refused (literal IPs skip DNS).
/// A client like [`guarded_client`] that returns redirects instead of following them.
pub fn guarded_no_redirect_client(ua: &str, connect: std::time::Duration) -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(ua).connect_timeout(connect).redirect(reqwest::redirect::Policy::none()).dns_resolver(std::sync::Arc::new(PublicResolver)).build()?)
}

pub fn guarded_client(ua: &str, connect: std::time::Duration, total: Option<std::time::Duration>) -> anyhow::Result<reqwest::Client> {
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("too many redirects");
        }
        let url = attempt.url();
        let private_literal = match url.host() {
            Some(url::Host::Ipv4(ip)) => !v4_is_public(ip),
            Some(url::Host::Ipv6(ip)) => !ip_is_public(IpAddr::V6(ip)),
            Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
            None => true,
        };
        if private_literal || !matches!(url.scheme(), "http" | "https") {
            attempt.stop()
        } else {
            attempt.follow()
        }
    });
    let mut b = reqwest::Client::builder().user_agent(ua).connect_timeout(connect).redirect(policy).dns_resolver(std::sync::Arc::new(PublicResolver));
    if let Some(t) = total {
        b = b.timeout(t);
    }
    Ok(b.build()?)
}

/// True when a URL's host is a literal private address (literal IPs bypass the resolver).
pub fn literal_private(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => !v4_is_public(ip),
        Some(url::Host::Ipv6(ip)) => !ip_is_public(IpAddr::V6(ip)),
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_addresses() {
        for bad in ["127.0.0.1", "10.1.2.3", "192.168.0.10", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fe80::1", "fd00::1", "::ffff:127.0.0.1", "64:ff9b::a00:1"] {
            assert!(!ip_is_public(bad.parse().unwrap()), "{bad} must be blocked");
        }
        for good in ["1.1.1.1", "142.250.180.14", "2606:4700:4700::1111"] {
            assert!(ip_is_public(good.parse().unwrap()), "{good} must be allowed");
        }
    }

    #[tokio::test]
    async fn refused_addresses_all_read_the_same() {
        let private = public_address(&Url::parse("http://localhost:9/").unwrap()).await.unwrap_err();
        let missing = public_address(&Url::parse("http://no-such-host.invalid/").unwrap()).await.unwrap_err();
        assert_eq!(private, missing);
    }
}
