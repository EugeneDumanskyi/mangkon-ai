# ADR-0012: API keys in the OS secret store

- **Status:** Proposed
- **Date:** 2026-10-12
- **Related:** NFR-SEC-07/08, NFR-COMP-01/07, INV-EG-07, US-09.6, domain §5.2 (ENDPOINT.secretRef), ADR-0009

## Context

- External endpoints may need an API key (US-09.6).
- Keys are stored in the OS keychain only. Never in the project DB, config files, or logs (NFR-SEC-07, INV-EG-07).
- Projects are portable (NFR-COMP-07). A copied project must not carry keys.
- Supported OSes: macOS, Windows, Ubuntu (NFR-COMP-01).

## Options considered

**1. OS secret store through the `keyring` crate** (macOS Keychain, Windows Credential Manager, Linux Secret Service).
- ➕ Meets NFR-SEC-07 directly. The OS protects the key with the user's login.
- ➕ One small, widely used crate for all three OSes.
- ➖ Some minimal Linux setups have no Secret Service running.

**2. Encrypted file with a master password.**
- ➕ Works everywhere.
- ➖ The user must type a password. More code and more risk (our own crypto handling). Not what NFR-SEC-07 asks for.

**3. Plain config file.**
- ➖ Breaks NFR-SEC-07.

## Decision

Use **option 1**.

- The app-level `Endpoint` stores only `secretRef` (service `mangkon-ai`, account = endpoint ID). Never the key itself.
- The key is read at send time by the egress gate (ADR-0009), kept in memory only as long as needed, and wrapped in a type that never prints its value (`secrecy` or similar).
- **No Secret Service on Linux:** the app says so in plain words. The user can enter the key for this session only. It is kept in memory and lost on quit. It is never written to disk.
- Logs mask `Authorization` headers and anything that looks like a key (NFR-SEC-08).
- Deleting an endpoint deletes its secret.

## Consequences

**Easier**
- No key handling in project files. Copied projects are safe to share.
- A CI test can grep the project folder and logs for a known test key (NFR-SEC-07).

**Harder**
- Linux users without a Secret Service must re-enter keys each session.
- On a new machine, the user enters keys again. This is intended.

**Follow-ups**
- Test on Ubuntu 22.04 with GNOME Keyring, and on a setup without it. **M5a:** `keyring` 3.6 (`sync-secret-service` + `crypto-rust` on Linux, `apple-native`, `windows-native`); service `mangkon-ai`, account `endpoint:<id>`; the store is probed on first use by reading an entry that is not there (no write, no unlock prompt). With no Secret Service, keys are kept in memory and the AI models screen shows the notice; after a restart the endpoint shows "API key missing: enter it again". Covered by `crates/mk-core/tests/m5a_egress.rs` with an in-memory keychain and a session store; the runs on Linux with the real keychain and with none are in `13-model-manager.md` §3.
