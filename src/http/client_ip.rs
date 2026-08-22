//! Who the request is actually from, behind however many proxies.
//!
//! The immediate peer of a TCP connection is whatever opened it. In a
//! deployment with a load balancer, a CDN, or an ingress controller in front
//! of the application, that is the last proxy and never the client -- so
//! anything that identifies a caller by peer address (rate limiting, bans,
//! abuse heuristics, an audit line) sees one address for the whole internet.
//!
//! The forwarding headers exist to fix that, and using them naively is worse
//! than not using them at all. `X-Forwarded-For` is a request header: a
//! client can send one. If the application believes it unconditionally, then
//! every per-IP limit and every ban is bypassed by a header the attacker
//! chooses, and the bypass is invisible in the logs because the logs believe
//! the header too.
//!
//! # The rule
//!
//! A forwarding header is evidence only when the hop that delivered it is
//! one we trust to have written it. [`ClientIp::resolve`] therefore starts
//! from the peer address and consults `X-Forwarded-For` **only** when the
//! peer is in the operator's [`TrustedProxies`] list. It then walks the
//! chain from the right -- the end the closest proxy appended to -- skipping
//! hops that are themselves trusted, and stops at the first address that is
//! not. That address was written by a trusted proxy about a party we do not
//! trust, which is the definition of the client.
//!
//! Everything to the left of that point may have been forged by the client
//! and is never read. That is what "skip exactly the right number of hops"
//! buys over "take the leftmost entry", which is the classic bypass.
//!
//! # The list is empty by default
//!
//! [`TrustedProxies::none`] is the default, and it means the resolved client
//! IP is always the peer address. An application that is genuinely behind a
//! proxy has to say which one:
//!
//! ```
//! use arcature::http::{ProxyNet, TrustedProxies};
//!
//! let trusted: TrustedProxies = "10.0.0.0/8, 127.0.0.1"
//!     .parse()
//!     .expect("two well-formed entries");
//! assert!(trusted.contains("10.4.1.9".parse::<std::net::IpAddr>().unwrap()));
//! assert!(!trusted.contains("203.0.113.7".parse::<std::net::IpAddr>().unwrap()));
//! # let _: ProxyNet = "10.0.0.0/8".parse().unwrap();
//! ```
//!
//! Defaulting to "trust the private ranges" would be convenient and wrong:
//! on a flat network, or in a container platform where another tenant's pod
//! shares the range, a private source address is not a proxy.
//!
//! # Where it is resolved
//!
//! Once, in the same per-connection layer that installs
//! [`ConnectInfo`](axum::extract::ConnectInfo) --
//! `ServeTarget::serve_with_trusted_proxies`. The result is a [`ClientIp`]
//! request extension, so every reader downstream sees the same answer and
//! no reader re-derives it from headers on its own.
//!
//! ```
//! use arcature::http::ClientIp;
//! use arcature::axum::Extension;
//!
//! async fn handler(Extension(client): Extension<ClientIp>) -> String {
//!     client.to_string()
//! }
//! ```

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use axum::http::{HeaderMap, HeaderName};

/// The `X-Forwarded-For` header name.
///
/// The de-facto standard rather than RFC 7239's `Forwarded`, because it is
/// what the proxies in front of real deployments actually write.
pub const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");

// ---------------------------------------------------------------------------
// ProxyNet
// ---------------------------------------------------------------------------

/// One entry in a [`TrustedProxies`] list: an address, or a CIDR block.
///
/// A block rather than only single addresses because a proxy tier is
/// normally a subnet whose members come and go, and an operator who has to
/// enumerate them will either get it wrong or give up and trust everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ProxyNet {
    /// The network address, already masked to `prefix` bits.
    base: IpAddr,
    /// The prefix length, in bits. At most 32 for IPv4, 128 for IPv6.
    prefix: u8,
}

impl ProxyNet {
    /// A block of `prefix` bits around `base`.
    ///
    /// Host bits in `base` are cleared rather than rejected: `10.0.0.1/8`
    /// and `10.0.0.0/8` describe the same set, and refusing the first would
    /// only turn a harmless typo into a boot failure.
    ///
    /// # Errors
    ///
    /// [`ProxyNetError::Prefix`] if `prefix` is longer than the address
    /// family allows.
    pub fn new(base: IpAddr, prefix: u8) -> Result<Self, ProxyNetError> {
        let base = base.to_canonical();
        let width = Self::width(base);
        if prefix > width {
            return Err(ProxyNetError::Prefix { prefix, width });
        }
        Ok(Self {
            base: Self::mask(base, prefix),
            prefix,
        })
    }

    /// A block containing exactly one address (`/32`, or `/128` for IPv6).
    #[must_use]
    pub fn host(addr: IpAddr) -> Self {
        let base = addr.to_canonical();
        Self {
            prefix: Self::width(base),
            base,
        }
    }

    /// Whether `addr` falls inside this block.
    ///
    /// An IPv4-mapped IPv6 address (`::ffff:198.51.100.4`) is compared as
    /// the IPv4 address it carries. A dual-stack listener reports one of
    /// those for every IPv4 client, and an operator who wrote `10.0.0.0/8`
    /// means that host either way.
    #[must_use]
    pub fn contains(&self, addr: IpAddr) -> bool {
        let addr = addr.to_canonical();
        // A v4 block never matches a v6 host, and vice versa: they are
        // different address spaces and the bit patterns are not comparable.
        if self.base.is_ipv4() != addr.is_ipv4() {
            return false;
        }
        Self::mask(addr, self.prefix) == self.base
    }

    /// The number of bits in an address of this family.
    fn width(addr: IpAddr) -> u8 {
        if addr.is_ipv4() { 32 } else { 128 }
    }

    /// `addr` with everything below `prefix` cleared.
    fn mask(addr: IpAddr, prefix: u8) -> IpAddr {
        match addr {
            IpAddr::V4(v4) => {
                let bits = u32::from(v4);
                let kept = if prefix == 0 {
                    0
                } else {
                    bits & (u32::MAX << (32 - u32::from(prefix)))
                };
                IpAddr::V4(kept.into())
            }
            IpAddr::V6(v6) => {
                let bits = u128::from(v6);
                let kept = if prefix == 0 {
                    0
                } else {
                    bits & (u128::MAX << (128 - u32::from(prefix)))
                };
                IpAddr::V6(kept.into())
            }
        }
    }
}

impl fmt::Display for ProxyNet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.base, self.prefix)
    }
}

impl From<IpAddr> for ProxyNet {
    fn from(addr: IpAddr) -> Self {
        Self::host(addr)
    }
}

impl FromStr for ProxyNet {
    type Err = ProxyNetError;

    /// Parse `10.0.0.0/8`, `2001:db8::/32`, or a bare address.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let (addr, prefix) = match text.split_once('/') {
            Some((addr, prefix)) => (addr.trim(), Some(prefix.trim())),
            None => (text, None),
        };
        let addr: IpAddr = addr
            .parse()
            .map_err(|_| ProxyNetError::Address(addr.to_string()))?;
        match prefix {
            None => Ok(Self::host(addr)),
            Some(prefix) => {
                let bits: u8 = prefix
                    .parse()
                    .map_err(|_| ProxyNetError::Address(text.to_string()))?;
                Self::new(addr, bits)
            }
        }
    }
}

/// Why a trusted-proxy entry could not be read.
///
/// A configuration mistake here silently disables the protection the list
/// exists to provide, so it is an error the application can refuse to boot
/// on rather than a value quietly dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProxyNetError {
    /// The text before the `/` is not an IP address.
    Address(String),
    /// The prefix length is longer than the address family allows.
    Prefix {
        /// What was asked for.
        prefix: u8,
        /// The most this family permits: 32 for IPv4, 128 for IPv6.
        width: u8,
    },
}

impl fmt::Display for ProxyNetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Address(text) => {
                write!(formatter, "`{text}` is not an IP address or CIDR block")
            }
            Self::Prefix { prefix, width } => write!(
                formatter,
                "a /{prefix} prefix does not fit an address of {width} bits"
            ),
        }
    }
}

impl std::error::Error for ProxyNetError {}

// ---------------------------------------------------------------------------
// TrustedProxies
// ---------------------------------------------------------------------------

/// The hops whose forwarding headers are believed.
///
/// Empty by default. See the [module documentation](self) for why the empty
/// list is the only safe default and what the list is used for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TrustedProxies {
    nets: Vec<ProxyNet>,
}

impl TrustedProxies {
    /// Trust nothing: the client IP is always the immediate peer.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Add one address or block to the list.
    #[must_use]
    pub fn trust(mut self, net: impl Into<ProxyNet>) -> Self {
        self.nets.push(net.into());
        self
    }

    /// Whether the list is empty -- that is, whether forwarding headers are
    /// ignored outright.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nets.is_empty()
    }

    /// The entries, in the order they were added.
    #[must_use]
    pub fn nets(&self) -> &[ProxyNet] {
        &self.nets
    }

    /// Whether `addr` is one of the trusted hops.
    #[must_use]
    pub fn contains(&self, addr: IpAddr) -> bool {
        self.nets.iter().any(|net| net.contains(addr))
    }
}

impl FromIterator<ProxyNet> for TrustedProxies {
    fn from_iter<I: IntoIterator<Item = ProxyNet>>(iter: I) -> Self {
        Self {
            nets: iter.into_iter().collect(),
        }
    }
}

impl FromStr for TrustedProxies {
    type Err = ProxyNetError;

    /// Parse a comma- or whitespace-separated list, for a value that arrived
    /// as one string. Empty text is the empty list.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.split([',', ' ', '\t', '\n'])
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(ProxyNet::from_str)
            .collect::<Result<Vec<_>, _>>()
            .map(|nets| Self { nets })
    }
}

// ---------------------------------------------------------------------------
// ClientIp
// ---------------------------------------------------------------------------

/// The address the request is attributed to, resolved once per request.
///
/// Present as a request extension on the TCP serve path, so a handler reads
/// it with `Extension<ClientIp>` and a Tower layer reads it out of
/// `extensions()`. Absent over IPC, where there is no peer address at all.
///
/// It is personal data. The access log records it as a structured field,
/// which is what puts it under the `observe::redact` deny-list rather than
/// beyond the reach of any formatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ClientIp(IpAddr);

impl ClientIp {
    /// Resolve the client address from the peer and the request headers.
    ///
    /// `trusted` decides whether `X-Forwarded-For` is read at all. See the
    /// [module documentation](self) for the walk and why it runs from the
    /// right.
    #[must_use]
    pub fn resolve(peer: IpAddr, headers: &HeaderMap, trusted: &TrustedProxies) -> Self {
        let peer = peer.to_canonical();
        // The fast path, and the only path for an application that named no
        // proxies: the peer opened the connection, so nothing it *sent* can
        // change who it is.
        if !trusted.contains(peer) {
            return Self(peer);
        }

        // `X-Forwarded-For` may arrive as several header lines; they
        // concatenate, in order, into one chain. `get_all` preserves that
        // order.
        let chain: Vec<&str> = headers
            .get_all(X_FORWARDED_FOR)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .collect();

        // Right to left: the rightmost entry was written by the hop closest
        // to us, which is the only entry that hop is in a position to vouch
        // for. `nearest` tracks the innermost address we have accepted so
        // far, so an all-trusted chain resolves to its leftmost entry.
        let mut nearest = peer;
        for entry in chain.into_iter().rev() {
            let Some(addr) = parse_forwarded(entry) else {
                // Not an address: `unknown`, an RFC 7239 obfuscated
                // identifier, or something hostile. We cannot attribute the
                // request past a hop we cannot read, and guessing would step
                // straight into client-controlled text.
                break;
            };
            if trusted.contains(addr) {
                nearest = addr;
                continue;
            }
            return Self(addr);
        }
        Self(nearest)
    }

    /// The address itself.
    #[must_use]
    pub fn addr(&self) -> IpAddr {
        self.0
    }
}

impl fmt::Display for ClientIp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<ClientIp> for IpAddr {
    fn from(client: ClientIp) -> Self {
        client.0
    }
}

/// Read one `X-Forwarded-For` entry as an address.
///
/// Proxies are not consistent about the form. Bare addresses are the common
/// case; a port is appended by some (Azure always, others for IPv6), and an
/// IPv6 literal may or may not carry brackets.
fn parse_forwarded(entry: &str) -> Option<IpAddr> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    if let Ok(addr) = entry.parse::<IpAddr>() {
        return Some(addr.to_canonical());
    }
    if let Ok(socket) = entry.parse::<SocketAddr>() {
        return Some(socket.ip().to_canonical());
    }
    entry
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|inner| inner.parse::<IpAddr>().ok())
        .map(|addr| addr.to_canonical())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("a literal address")
    }

    fn headers(chain: &[&str]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for value in chain {
            map.append(
                X_FORWARDED_FOR,
                value.parse().expect("a literal header value"),
            );
        }
        map
    }

    #[test]
    fn a_block_masks_its_host_bits() {
        let net: ProxyNet = "10.4.1.9/8".parse().expect("a literal block");
        assert_eq!(net.to_string(), "10.0.0.0/8");
        assert!(net.contains(ip("10.255.255.255")));
        assert!(!net.contains(ip("11.0.0.1")));
    }

    #[test]
    fn a_bare_address_is_a_single_host() {
        let net: ProxyNet = "192.0.2.5".parse().expect("a literal address");
        assert_eq!(net.to_string(), "192.0.2.5/32");
        assert!(net.contains(ip("192.0.2.5")));
        assert!(!net.contains(ip("192.0.2.6")));
    }

    #[test]
    fn the_two_address_families_do_not_match_each_other() {
        let v4: ProxyNet = "0.0.0.0/0".parse().expect("a literal block");
        assert!(v4.contains(ip("203.0.113.1")));
        assert!(!v4.contains(ip("2001:db8::1")));

        let v6: ProxyNet = "2001:db8::/32".parse().expect("a literal block");
        assert!(v6.contains(ip("2001:db8::dead:beef")));
        assert!(!v6.contains(ip("2001:db9::1")));
    }

    #[test]
    fn a_mapped_v4_address_is_compared_as_v4() {
        let net: ProxyNet = "10.0.0.0/8".parse().expect("a literal block");
        assert!(net.contains(ip("::ffff:10.1.2.3")));
    }

    #[test]
    fn an_over_long_prefix_is_refused() {
        assert_eq!(
            "10.0.0.0/33".parse::<ProxyNet>(),
            Err(ProxyNetError::Prefix {
                prefix: 33,
                width: 32
            })
        );
        assert!(matches!(
            "not-an-address/8".parse::<ProxyNet>(),
            Err(ProxyNetError::Address(_))
        ));
    }

    #[test]
    fn a_list_parses_from_one_string() {
        let trusted: TrustedProxies = "10.0.0.0/8, 127.0.0.1"
            .parse()
            .expect("two literal entries");
        assert_eq!(trusted.nets().len(), 2);
        assert!(trusted.contains(ip("10.9.9.9")));
        assert!(trusted.contains(ip("127.0.0.1")));
        assert!(!trusted.contains(ip("203.0.113.1")));

        assert!(
            "".parse::<TrustedProxies>()
                .expect("empty is a list")
                .is_empty()
        );
    }

    /// The bypass this module exists to prevent.
    #[test]
    fn an_untrusted_peer_cannot_forge_a_forwarded_header() {
        let resolved = ClientIp::resolve(
            ip("203.0.113.9"),
            &headers(&["1.2.3.4"]),
            &TrustedProxies::none(),
        );
        assert_eq!(resolved.addr(), ip("203.0.113.9"));
    }

    #[test]
    fn a_trusted_proxy_hands_over_the_client() {
        let trusted = TrustedProxies::none().trust(ip("10.0.0.1"));
        let resolved = ClientIp::resolve(ip("10.0.0.1"), &headers(&["203.0.113.9"]), &trusted);
        assert_eq!(resolved.addr(), ip("203.0.113.9"));
    }

    /// Two trusted hops, and a client that prefixed the chain with a lie.
    /// The walk from the right stops at the first untrusted entry, so the
    /// forged prefix is never reached.
    #[test]
    fn the_walk_stops_at_the_first_untrusted_hop() {
        let trusted: TrustedProxies = "10.0.0.0/8".parse().expect("a literal block");
        let resolved = ClientIp::resolve(
            ip("10.0.0.1"),
            &headers(&["9.9.9.9, 203.0.113.9, 10.0.0.2"]),
            &trusted,
        );
        assert_eq!(resolved.addr(), ip("203.0.113.9"));
    }

    #[test]
    fn several_header_lines_are_one_chain() {
        let trusted: TrustedProxies = "10.0.0.0/8".parse().expect("a literal block");
        let resolved = ClientIp::resolve(
            ip("10.0.0.1"),
            &headers(&["203.0.113.9", "10.0.0.2"]),
            &trusted,
        );
        assert_eq!(resolved.addr(), ip("203.0.113.9"));
    }

    /// Nothing in the chain is a client, so the leftmost trusted hop is the
    /// closest thing to an answer -- and it is still an address a trusted
    /// proxy wrote.
    #[test]
    fn an_all_trusted_chain_resolves_to_its_leftmost_entry() {
        let trusted: TrustedProxies = "10.0.0.0/8".parse().expect("a literal block");
        let resolved =
            ClientIp::resolve(ip("10.0.0.1"), &headers(&["10.0.0.7, 10.0.0.2"]), &trusted);
        assert_eq!(resolved.addr(), ip("10.0.0.7"));
    }

    #[test]
    fn an_unreadable_hop_ends_the_walk() {
        let trusted: TrustedProxies = "10.0.0.0/8".parse().expect("a literal block");
        let resolved = ClientIp::resolve(
            ip("10.0.0.1"),
            &headers(&["203.0.113.9, unknown, 10.0.0.2"]),
            &trusted,
        );
        assert_eq!(
            resolved.addr(),
            ip("10.0.0.2"),
            "an entry we cannot read must not let the one behind it through"
        );
    }

    #[test]
    fn a_hop_with_a_port_is_still_an_address() {
        let trusted: TrustedProxies = "10.0.0.0/8".parse().expect("a literal block");
        for entry in ["203.0.113.9:4711", "[2001:db8::1]:443", "[2001:db8::1]"] {
            let resolved = ClientIp::resolve(ip("10.0.0.1"), &headers(&[entry]), &trusted);
            assert_ne!(
                resolved.addr(),
                ip("10.0.0.1"),
                "`{entry}` should have parsed"
            );
        }
    }

    #[test]
    fn a_trusted_peer_with_no_header_stays_the_peer() {
        let trusted: TrustedProxies = "10.0.0.0/8".parse().expect("a literal block");
        let resolved = ClientIp::resolve(ip("10.0.0.1"), &HeaderMap::new(), &trusted);
        assert_eq!(resolved.addr(), ip("10.0.0.1"));
    }
}
