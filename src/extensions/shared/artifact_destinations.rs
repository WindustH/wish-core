//! Artifact URLs are untrusted provider output. Only the explicitly configured service
//! origin may use a local address; other destinations require public HTTPS. The configured
//! proxy remains a trusted network hop and is responsible for its own DNS/egress policy.
use std::net::IpAddr;
pub(super) fn public(ip: IpAddr) -> bool {
  match ip {
    IpAddr::V4(ip) => {
      let [a, b, c, _] = ip.octets();
      !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || a == 0
        || a >= 240
        || (a == 100 && (64..=127).contains(&b))
        || (a == 198 && (b == 18 || b == 19))
        || (a == 192 && b == 0 && c == 0))
    }
    IpAddr::V6(ip) => {
      if let Some(v4) = ip.to_ipv4_mapped() {
        return public(IpAddr::V4(v4));
      }
      let segments = ip.segments();
      segments[0] & 0xe000 == 0x2000
        && segments[0] != 0x2002
        && !(segments[0] == 0x2001 && (segments[1] == 0xdb8 || segments[1] == 0))
    }
  }
}
pub(super) fn trusted(url: &reqwest::Url, base: &str) -> bool {
  super::urls::validate_url(base).is_ok_and(|base| base.origin() == url.origin())
}
pub(super) fn validate(value: &str, base: &str) -> Result<reqwest::Url, String> {
  let url = super::urls::validate_url(value).map_err(|e| e.to_string())?;
  if value.len() > 8192 || url.fragment().is_some() {
    return Err("invalid artifact URL".into());
  }
  if trusted(&url, base) {
    return Ok(url);
  }
  let hostname = url.host_str().unwrap_or_default().trim_end_matches('.').to_ascii_lowercase();
  if url.scheme() != "https"
    || hostname == "localhost"
    || hostname.ends_with(".localhost")
    || hostname.ends_with(".local")
    || hostname.trim_matches(['[', ']']).parse::<IpAddr>().is_ok_and(|ip| !public(ip))
  {
    return Err("artifact URL must use public HTTPS or the configured service origin".into());
  }
  Ok(url)
}
pub(super) struct PublicDns;
impl reqwest::dns::Resolve for PublicDns {
  fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
    Box::pin(async move {
      let addresses: Vec<_> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
      if addresses.is_empty() || addresses.iter().any(|address| !public(address.ip())) {
        return Err(
          std::io::Error::other("artifact hostname resolves to a non-public address").into(),
        );
      }
      Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
    })
  }
}
