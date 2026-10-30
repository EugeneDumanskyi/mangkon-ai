//! Registrable domains (US-04.3, INV-ID-05).

/// The registrable domain of a host by the full Public Suffix List, lower case:
/// `www.shell-co.example` → `shell-co.example`, `edis.usitc.gov` → `usitc.gov`, `tc.gc.ca` →
/// `tc.gc.ca`. A host under a suffix the list does not know counts its last label as the suffix
/// (the list's `*` rule), so `x.example` stays `x.example`. `None` for a public suffix itself or a
/// host with one label.
pub fn registrable_domain(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_lowercase();
    if !host.contains('.') {
        return None;
    }
    psl::domain_str(&host).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// US-04.3 AC1: PSL, not "drop www.": `example.org` is registrable, so a host under it gives
    /// `example.org`.
    #[test]
    fn us_04_3_ac1_registrable_domain_by_psl() {
        for (host, want) in [
            ("www.shell-co.example", Some("shell-co.example")),
            ("WWW.Shell-Co.Example", Some("shell-co.example")),
            ("www.shell-co.example.org", Some("example.org")),
            ("edis.usitc.gov", Some("usitc.gov")),
            ("tc.gc.ca", Some("tc.gc.ca")),
            ("abc.airbus", Some("abc.airbus")),
            ("x.example", Some("x.example")),
            ("a.b.c.co.uk", Some("c.co.uk")),
            ("co.uk", None),
            ("localhost", None),
        ] {
            assert_eq!(registrable_domain(host).as_deref(), want, "{host}");
        }
    }
}
