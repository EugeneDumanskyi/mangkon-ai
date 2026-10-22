# ADR-0009: One egress gate for all model traffic

- **Status:** Proposed
- **Date:** 2026-10-12
- **Related:** NFR-SEC-01..07, INV-EG-01..08, INV-PR-09, INV-SR-03, domain §2.7, §7.6, BC-11, ADR-0003, ADR-0008

## Context

- Privacy is the main reason people choose this app. Project data may only go to "This machine", or to a non-local endpoint the user **confirmed for this project** with its exact host and class (INV-EG-01..04).
- New projects allow no non-local endpoint (NFR-SEC-04).
- Every destination that received data is recorded (INV-EG-05). The current destination is always shown (NFR-SEC-05).
- Meaning-search **queries** also leave the app (INV-SR-03).
- The class comes from the resolved address. If a host resolves to several classes, the most exposed wins (INV-EG-03).
- No telemetry. The only other network use is model and runtime downloads (NFR-SEC-01/02).

## Options considered

**1. Convention: each caller checks permission before it sends.**
- ➕ No extra structure.
- ➖ One forgotten check leaks data. Hard to review in an open-source project with many contributors.

**2. A type-level choke point.** Only the egress crate owns an HTTP client for project data. Callers can get a model client **only** from the gate.
- ➕ A missing check is a compile error, not a bug.
- ➕ One place to test, audit, and record.
- ➖ The gate must be designed carefully (DNS, retries, streaming).

**3. OS-level filtering (local proxy or firewall rules).**
- ➕ Catches everything.
- ➖ Different on every OS. Needs admin rights. Not possible in a source-built desktop app.

## Decision

Use **option 2**.

**Structure**
- `mk-egress` is the only crate that may create an HTTP client for project data. `mk-models` may create one for downloads only.
- `cargo-deny` enforces this in CI: the HTTP client crate is banned except as a dependency of `mk-egress` and `mk-models` (the `wrappers` setting).
- The webview cannot make network requests (CSP and no `http` plugin, ADR-0003).

**Per request, the gate does:**
1. Look up the destination: local runtime (always loopback) or an app-level endpoint.
2. Resolve the host to **all** its addresses. Classify each: loopback → This machine; private ranges → Local network; everything else → Internet. Take the most exposed class (INV-EG-03).
3. Check the project's permission (domain §7.6). Allowed if the class is This machine, or the permission is `Allowed` with the **same** host and class. A more exposed class than confirmed → `NeedsReconfirmation`; the job becomes Blocked (INV-PR-09).
4. Record: upsert the `EgressRecord` (host, class, purpose, first and last sent) in the project store **before** sending (INV-EG-05).
5. Send using a client **pinned to the address that was checked**. This stops a DNS change between the check and the send.
6. Publish the current destination for the status bar (NFR-SEC-05).

**Other rules**
- Endpoint tests send a fixed test text, never project content (INV-EG-08).
- API keys are read from the OS secret store at send time and never logged (ADR-0012). Logs mask the `Authorization` header.
- Downloads (`mk-models`) only go to hosts listed in the catalog. They never carry project data.

## Consequences

**Easier**
- A contributor cannot send project data by accident. The compiler and CI stop it.
- One module to audit for the privacy promise. One network-capture test for NFR-SEC-02.
- The "Data sent to …" list in project settings comes straight from `EgressRecord`.

**Harder**
- DNS is resolved on every request. The OS cache makes this cheap, but the gate must handle slow or failing DNS with a plain message.
- Pinning to an address must still use the hostname for TLS (SNI and certificate checks). The HTTP client must support "resolve this host to this address".

**Follow-ups**
- Tests: rebinding (host changes from LAN to internet between calls), multi-address hosts, revoked permission during a running job.
- Network-capture test on a full TD-1 run (NFR-SEC-02/03).
