//! The egress rules, pure (no I/O): address classes, origins and the permission check
//! (US-09.7 AC4, INV-EG-03/04, domain §7.6).

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::Url;

/// Where an address is, from least to most exposed (INV-EG-03).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DestinationClass {
    ThisMachine,
    LocalNetwork,
    Internet,
}

impl DestinationClass {
    /// The stored form.
    pub fn as_str(self) -> &'static str {
        match self {
            DestinationClass::ThisMachine => "this_machine",
            DestinationClass::LocalNetwork => "local_network",
            DestinationClass::Internet => "internet",
        }
    }

    pub fn parse(s: &str) -> Option<DestinationClass> {
        Some(match s {
            "this_machine" => DestinationClass::ThisMachine,
            "local_network" => DestinationClass::LocalNetwork,
            "internet" => DestinationClass::Internet,
            _ => return None,
        })
    }

    /// The words the UI shows (US-09.8 AC1).
    pub fn label(self) -> &'static str {
        match self {
            DestinationClass::ThisMachine => "this machine",
            DestinationClass::LocalNetwork => "local network",
            DestinationClass::Internet => "internet",
        }
    }
}

/// An address the gate will not send to at all: `0.0.0.0` or `::` names no host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unspecified;

fn classify_v4(ip: Ipv4Addr) -> Result<DestinationClass, Unspecified> {
    if ip.is_unspecified() {
        return Err(Unspecified);
    }
    let [a, b, ..] = ip.octets();
    Ok(if ip.is_loopback() {
        DestinationClass::ThisMachine
    } else if a == 10 || (a == 172 && (16..=31).contains(&b)) || (a == 192 && b == 168) {
        DestinationClass::LocalNetwork
    } else {
        // Link-local (169.254/16) and shared address space (100.64/10, used by some VPNs) are
        // not in US-09.7 AC4's list, so they count as Internet.
        DestinationClass::Internet
    })
}

fn classify_v6(ip: Ipv6Addr) -> Result<DestinationClass, Unspecified> {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return classify_v4(v4);
    }
    if ip.is_unspecified() {
        return Err(Unspecified);
    }
    Ok(if ip.is_loopback() {
        DestinationClass::ThisMachine
    } else if (ip.segments()[0] & 0xfe00) == 0xfc00 {
        // fc00::/7, unique local addresses.
        DestinationClass::LocalNetwork
    } else {
        DestinationClass::Internet
    })
}

/// The class of one address, exactly as US-09.7 AC4 lists it: loopback → This machine;
/// 10/8, 172.16/12, 192.168/16, fc00::/7 → Local network; everything else → Internet. An
/// IPv4-mapped IPv6 address is classified as its IPv4 address.
pub fn classify(ip: IpAddr) -> Result<DestinationClass, Unspecified> {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

/// The most exposed class of a host's addresses (INV-EG-03). `None` for no addresses.
pub fn most_exposed(ips: &[IpAddr]) -> Result<Option<DestinationClass>, Unspecified> {
    let mut worst = None;
    for ip in ips {
        let class = classify(*ip)?;
        worst = worst.max(Some(class));
    }
    Ok(worst)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

/// Scheme, host and port: what a permission is confirmed for. The host is lower case, IDNA
/// encoded, and an IPv6 address without brackets.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Origin {
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
}

impl Origin {
    /// The host as an address, when it is one (no DNS needed).
    pub fn ip(&self) -> Option<IpAddr> {
        self.host.parse().ok()
    }

    /// `host:port`, with brackets around an IPv6 address.
    pub fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// The stored form, `scheme://host:port`, the port always written.
    pub fn key(&self) -> String {
        format!("{}://{}", self.scheme.as_str(), self.authority())
    }

    /// `scheme://host[:port]`, the port left out when it is the scheme's default.
    pub fn url_prefix(&self) -> String {
        let default = match self.scheme {
            Scheme::Http => 80,
            Scheme::Https => 443,
        };
        if self.port == default {
            let host = if self.host.contains(':') {
                format!("[{}]", self.host)
            } else {
                self.host.clone()
            };
            format!("{}://{host}", self.scheme.as_str())
        } else {
            self.key()
        }
    }

    pub fn parse_key(key: &str) -> Option<Origin> {
        let url = Url::parse(key).ok()?;
        origin_of(&url).ok()
    }

    /// Plain `http`: the key and the data go unencrypted.
    pub fn encrypted(&self) -> bool {
        self.scheme == Scheme::Https
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

/// Why a base URL was refused, in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadUrl(pub String);

impl fmt::Display for BadUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn origin_of(url: &Url) -> Result<Origin, BadUrl> {
    let scheme = match url.scheme() {
        "http" => Scheme::Http,
        "https" => Scheme::Https,
        _ => return Err(BadUrl("The URL must start with http:// or https://".into())),
    };
    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.trim_end_matches('.').to_ascii_lowercase(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        None => return Err(BadUrl("The URL has no host".into())),
    };
    if host.is_empty() {
        return Err(BadUrl("The URL has no host".into()));
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| BadUrl("The URL has no port".into()))?;
    Ok(Origin { scheme, host, port })
}

/// A checked endpoint base URL: http or https, a host, no user name or password (they would
/// end up in `app.db`, INV-EG-07), no query or fragment. A URL with no path gets `/v1`, the
/// OpenAI-compatible root; any other path is kept as typed. A bare host means the
/// `/v1` API (llama-server, Ollama and LM Studio all serve it there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseUrl {
    url: String,
    origin: Origin,
}

impl BaseUrl {
    pub fn parse(input: &str) -> Result<BaseUrl, BadUrl> {
        let input = input.trim();
        if input.is_empty() {
            return Err(BadUrl("Enter the endpoint's base URL".into()));
        }
        let url = Url::parse(input).map_err(|e| BadUrl(format!("Not a valid URL ({e})")))?;
        let origin = origin_of(&url)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(BadUrl(
                "Remove the user name and password from the URL; put an API key in the key field"
                    .into(),
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(BadUrl("The URL must not have a ? or # part".into()));
        }
        let path = url.path().trim_end_matches('/');
        let path = if path.is_empty() { "/v1" } else { path };
        let url = format!("{}{path}", origin.url_prefix());
        Ok(BaseUrl { url, origin })
    }

    /// The URL of the local runtime on `port` (always loopback, INV-MM-05).
    pub fn local_runtime(port: u16) -> BaseUrl {
        let origin = Origin {
            scheme: Scheme::Http,
            host: "127.0.0.1".into(),
            port,
        };
        BaseUrl {
            url: format!("{}/v1", origin.url_prefix()),
            origin,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.url
    }

    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// `<base>/<path>`, `path` without a leading slash.
    pub fn join(&self, path: &str) -> String {
        format!("{}/{path}", self.url)
    }
}

impl fmt::Display for BaseUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.url)
    }
}

/// Why a confirmed permission no longer holds (domain §7.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconfirmReason {
    /// The host now resolves to another class than the one confirmed (INV-EG-04).
    ClassChanged {
        confirmed: DestinationClass,
        now: DestinationClass,
    },
    /// The endpoint's URL now names another scheme, host or port.
    EndpointEdited,
    /// No endpoint with this id on this machine (deleted, or the project was copied here).
    EndpointNotFound,
    /// A reason stored earlier, as text.
    Stored(String),
}

impl fmt::Display for ReconfirmReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReconfirmReason::ClassChanged { confirmed, now } => write!(
                f,
                "The host was {} when you allowed it and is {} now",
                confirmed.label(),
                now.label()
            ),
            ReconfirmReason::EndpointEdited => {
                f.write_str("The endpoint's address was changed after you allowed it")
            }
            ReconfirmReason::EndpointNotFound => {
                f.write_str("The endpoint is not set up on this computer")
            }
            ReconfirmReason::Stored(s) => f.write_str(s),
        }
    }
}

/// A project's permission for one non-local endpoint (domain §7.6). No stored row is
/// [`Permission::NotAllowed`] (INV-EG-01). "Awaiting confirmation" is the confirm dialog, never
/// stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Permission {
    NotAllowed,
    Allowed {
        origin: Origin,
        class: DestinationClass,
        at_ms: i64,
    },
    NeedsReconfirmation {
        reason: String,
    },
}

/// What the check says about one send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Not allowed for this project; nothing is stored or changed.
    NotAllowed,
    /// A confirmed permission no longer holds; the job is Blocked (INV-PR-09).
    NeedsReconfirmation(ReconfirmReason),
}

/// The permission check (ADR-0009 step 3, INV-EG-04): This machine is always allowed;
/// otherwise only an `Allowed` permission for the same origin **and** the same class. Any other
/// class than the confirmed one needs a new confirmation, less exposed too (the domain rule
/// "same class", which is stricter than ADR-0009's "more exposed").
pub fn check(origin: &Origin, class: DestinationClass, permission: &Permission) -> Verdict {
    if class == DestinationClass::ThisMachine {
        return Verdict::Allow;
    }
    match permission {
        Permission::NotAllowed => Verdict::NotAllowed,
        Permission::NeedsReconfirmation { reason } => {
            Verdict::NeedsReconfirmation(ReconfirmReason::Stored(reason.clone()))
        }
        Permission::Allowed {
            origin: confirmed,
            class: confirmed_class,
            ..
        } => {
            if confirmed != origin {
                Verdict::NeedsReconfirmation(ReconfirmReason::EndpointEdited)
            } else if *confirmed_class != class {
                Verdict::NeedsReconfirmation(ReconfirmReason::ClassChanged {
                    confirmed: *confirmed_class,
                    now: class,
                })
            } else {
                Verdict::Allow
            }
        }
    }
}

/// What a project's data is sent for (domain EGRESS_RECORD.purpose, INV-SR-03).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Purpose {
    Extraction,
    Embeddings,
    SearchQuery,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Extraction => "extraction",
            Purpose::Embeddings => "embeddings",
            Purpose::SearchQuery => "search_query",
        }
    }

    pub fn parse(s: &str) -> Option<Purpose> {
        Some(match s {
            "extraction" => Purpose::Extraction,
            "embeddings" => Purpose::Embeddings,
            "search_query" => Purpose::SearchQuery,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::DestinationClass::*;
    use super::*;

    fn class(s: &str) -> Result<DestinationClass, Unspecified> {
        classify(s.parse().unwrap())
    }

    /// US-09.7 AC4: the ranges exactly as listed, IPv6 too.
    #[test]
    fn us_09_7_ac4_classification() {
        let table = [
            ("127.0.0.1", ThisMachine),
            ("127.255.0.9", ThisMachine),
            ("::1", ThisMachine),
            ("10.0.0.5", LocalNetwork),
            ("10.255.255.255", LocalNetwork),
            ("172.16.0.1", LocalNetwork),
            ("172.31.255.254", LocalNetwork),
            ("172.15.255.255", Internet),
            ("172.32.0.1", Internet),
            ("192.168.1.20", LocalNetwork),
            ("192.169.0.1", Internet),
            ("fc00::1", LocalNetwork),
            ("fd12:3456::1", LocalNetwork),
            ("fe00::1", Internet),
            ("8.8.8.8", Internet),
            ("93.184.215.14", Internet),
            ("2606:4700::1111", Internet),
            // Not in the AC's list: Internet (note in the plan).
            ("169.254.1.1", Internet),
            ("fe80::1", Internet),
            ("100.64.0.1", Internet),
            // IPv4-mapped IPv6 is its IPv4 address.
            ("::ffff:192.168.1.20", LocalNetwork),
            ("::ffff:127.0.0.1", ThisMachine),
            ("::ffff:8.8.8.8", Internet),
        ];
        for (ip, want) in table {
            assert_eq!(class(ip), Ok(want), "{ip}");
        }
        assert_eq!(class("0.0.0.0"), Err(Unspecified));
        assert_eq!(class("::"), Err(Unspecified));
        assert_eq!(class("::ffff:0.0.0.0"), Err(Unspecified));
    }

    /// INV-EG-03: the most exposed class wins.
    #[test]
    fn inv_eg_03_most_exposed_wins() {
        let ips: Vec<IpAddr> = ["10.0.0.5", "8.8.8.8", "127.0.0.1"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(most_exposed(&ips), Ok(Some(Internet)));
        assert_eq!(most_exposed(&ips[..1]), Ok(Some(LocalNetwork)));
        assert_eq!(most_exposed(&[]), Ok(None));
    }

    #[test]
    fn base_url_rules() {
        let b = BaseUrl::parse("http://192.168.1.20:8080").unwrap();
        assert_eq!(b.as_str(), "http://192.168.1.20:8080/v1");
        assert_eq!(b.origin().authority(), "192.168.1.20:8080");
        assert_eq!(
            BaseUrl::parse("https://API.Example.com/v1/")
                .unwrap()
                .as_str(),
            "https://api.example.com/v1"
        );
        assert_eq!(
            BaseUrl::parse("https://host.test/api/openai")
                .unwrap()
                .as_str(),
            "https://host.test/api/openai"
        );
        assert_eq!(
            BaseUrl::parse("http://[::1]:11434").unwrap().as_str(),
            "http://[::1]:11434/v1"
        );
        assert_eq!(
            BaseUrl::parse("http://[::1]:11434").unwrap().origin().host,
            "::1"
        );
        for bad in [
            "",
            "ftp://host/",
            "http://user:pw@host/",
            "http://host/v1?x=1",
            "not a url",
            "file:///etc/passwd",
        ] {
            assert!(BaseUrl::parse(bad).is_err(), "{bad}");
        }
        let o = BaseUrl::parse("https://h.test").unwrap().origin().clone();
        assert_eq!(Origin::parse_key(&o.key()), Some(o));
    }

    /// INV-EG-04, domain §7.6.
    #[test]
    fn inv_eg_04_check() {
        let origin = BaseUrl::parse("https://h.test").unwrap().origin().clone();
        let allowed = Permission::Allowed {
            origin: origin.clone(),
            class: LocalNetwork,
            at_ms: 0,
        };
        assert_eq!(
            check(&origin, ThisMachine, &Permission::NotAllowed),
            Verdict::Allow
        );
        assert_eq!(
            check(&origin, LocalNetwork, &Permission::NotAllowed),
            Verdict::NotAllowed
        );
        assert_eq!(check(&origin, LocalNetwork, &allowed), Verdict::Allow);
        assert_eq!(
            check(&origin, Internet, &allowed),
            Verdict::NeedsReconfirmation(ReconfirmReason::ClassChanged {
                confirmed: LocalNetwork,
                now: Internet
            })
        );
        let other = BaseUrl::parse("https://h.test:8443")
            .unwrap()
            .origin()
            .clone();
        assert_eq!(
            check(&other, LocalNetwork, &allowed),
            Verdict::NeedsReconfirmation(ReconfirmReason::EndpointEdited)
        );
        assert!(matches!(
            check(
                &origin,
                LocalNetwork,
                &Permission::NeedsReconfirmation { reason: "x".into() }
            ),
            Verdict::NeedsReconfirmation(_)
        ));
    }
}
