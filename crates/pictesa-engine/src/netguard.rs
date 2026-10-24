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
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.trim_start_matches('[').trim_end_matches(']'), port))
        .await
        .map_err(|_| "the host name does not resolve".to_string())?
        .collect();
    if addrs.is_empty() {
        return Err("the host name does not resolve".into());
    }
    // Every answer must be public, or DNS could alternate between a public and a private address.
    if addrs.iter().any(|a| !ip_is_public(a.ip())) {
        return Err("private and local network addresses are not allowed".into());
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
}
