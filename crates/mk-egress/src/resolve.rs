//! Name resolution, behind a port (ADR-0002 rule 2) so the tests can make a host change its
//! addresses between two calls (ADR-0009 follow-up: rebinding).

use std::net::{IpAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::time::Duration;

use crate::error::{EgressError, ErrorCode};
use crate::policy::{DestinationClass, Origin, Unspecified, most_exposed};

/// Resolves a host to **all** its addresses (ADR-0009 step 2).
pub trait Resolve: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, EgressError>;
}

/// The OS resolver (`getaddrinfo`, with the OS cache), on a helper thread so a slow DNS gives a
/// plain error after `timeout` instead of a hang.
#[derive(Clone, Debug)]
pub struct SystemResolver {
    pub timeout: Duration,
}

/// How long one lookup may take.
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);

impl Default for SystemResolver {
    fn default() -> Self {
        SystemResolver {
            timeout: RESOLVE_TIMEOUT,
        }
    }
}

impl Resolve for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, EgressError> {
        let (tx, rx) = mpsc::sync_channel(1);
        let name = host.to_string();
        std::thread::Builder::new()
            .name("mk-egress-dns".into())
            .spawn(move || {
                let r = (name.as_str(), port)
                    .to_socket_addrs()
                    .map(|it| it.map(|a| a.ip()).collect::<Vec<_>>());
                let _ = tx.send(r);
            })
            .map_err(|e| EgressError::new(ErrorCode::HostNotFound, e.to_string()))?;
        let not_found =
            || EgressError::new(ErrorCode::HostNotFound, format!("Cannot find host {host}"));
        match rx.recv_timeout(self.timeout) {
            Ok(Ok(mut ips)) if !ips.is_empty() => {
                ips.sort();
                ips.dedup();
                Ok(ips)
            }
            Ok(_) => Err(not_found()),
            Err(_) => Err(EgressError::new(
                ErrorCode::Timeout,
                format!("Cannot find host {host} (the name lookup timed out)"),
            )),
        }
    }
}

/// A host's addresses and their most exposed class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub ips: Vec<IpAddr>,
    pub class: DestinationClass,
}

/// Resolves `origin` (an address literal needs no DNS) and classifies it (INV-EG-03).
pub fn resolve_origin(resolver: &dyn Resolve, origin: &Origin) -> Result<Resolved, EgressError> {
    let ips = match origin.ip() {
        Some(ip) => vec![ip],
        None => resolver.resolve(&origin.host, origin.port)?,
    };
    match most_exposed(&ips) {
        Ok(Some(class)) => Ok(Resolved { ips, class }),
        Ok(None) => Err(EgressError::new(
            ErrorCode::HostNotFound,
            format!("Cannot find host {}", origin.host),
        )),
        Err(Unspecified) => Err(EgressError::new(
            ErrorCode::UnspecifiedAddress,
            format!(
                "{} points to 0.0.0.0 or ::, which is not a host to send to",
                origin.host
            ),
        )),
    }
}
