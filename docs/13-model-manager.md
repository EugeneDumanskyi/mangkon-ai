# 13 — Model manager and egress gate

> Input: `06-roadmap.md` §2 M5 (spike S5), ADR-0009, ADR-0012, ADR-0015, `02-requirements.md` E09, `03-nfr.md` (NFR-SEC-02/04/05/07, NFR-COMP-05), `04-domain.md` BC-11 and §7.6. **If this file disagrees with them, they win.**
> Status: §1 and §3 from M5a on RM-1 (2026-11-03). §2: llama-server on RM-1 done; Ollama, LM Studio and RM-2 wait for the maintainer's setup. Linux only; macOS and Windows are not verified here.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. Spike S5 — pinned address with TLS (K8)

### Question

Can the HTTP client send to the address the gate checked while keeping SNI and the certificate check for the host name? Do the three ADR-0009 tests block or reconfirm as specified (INV-EG-03, domain §7.6)?

**Pass mark:** all three block or reconfirm, and the pinned TLS call keeps SNI and the certificate check.

### Method

`ureq` 3.5.0 (pinned with `=`), built with `Agent::with_parts` and a resolver (`ureq::unversioned::resolver::Resolver`) that returns only the addresses the gate checked and refuses any other host. TLS by rustls 0.23 with the ring provider; the app trusts the OS store (`rustls-platform-verifier`), the tests trust only a test CA made with `rcgen`. Proxies off, redirects off (`max_redirects(0)`). The `rustls` feature of ureq would pull `webpki-roots` (CDLA-Permissive-2.0, not on the license allow list), so ureq is built with `rustls-no-provider` and the provider is passed in.

```sh
cargo test -p mk-egress --test s5_pinning
```

The tests run against an in-process fake OpenAI-compatible server (`mk-egress` feature `testkit`) over HTTP or TLS, with a scripted resolver. The rebinding and revoke tests serve on this machine's private address (found with a UDP connect, no packet sent), so the gate sends real requests to a Local network destination; with no private address they fall back to checking the gate alone and say so.

### Results

| Test | What it shows | Result |
|---|---|---|
| `s5_pinned_tls_keeps_sni_and_cert` | `https://api.example.test:<port>` pinned to 127.0.0.1: the call works, the server saw SNI `api.example.test`, and `.test` never resolved (no DNS was used) | Pass |
| `s5_pinned_tls_wrong_name_fails` | Certificate for `other.test` → `bad_certificate`, no request reached the server | Pass |
| `s5_untrusted_ca_fails` | Certificate from a CA not trusted → `bad_certificate` | Pass |
| `s5_send_uses_checked_address_not_a_second_lookup` | One lookup per call; the client never resolves again before sending | Pass |
| `s5_rebinding_lan_to_internet_needs_reconfirmation` | Host resolves to the local network, then to the internet: call 2 is `NeedsReconfirmation`, nothing sent, nothing recorded, the permission row moves to "needs reconfirmation" | Pass (real LAN path) |
| `s5_multi_address_most_exposed_wins` | `[10.0.0.5, 8.8.8.8]` → Internet (INV-EG-03) | Pass |
| `s5_revoked_during_running_job_blocks_next_call` | A job thread calls in a loop; a revoke in between blocks the next call and the server gets nothing after it | Pass (real LAN path) |
| `s5_redirect_not_followed` | A 302 to another host is the `redirect` error, not followed | Pass |
| `local_runtime_needs_no_permission_and_is_recorded` | The local runtime (loopback) needs no permission and is recorded | Pass |
| `endpoint_missing_or_edited_needs_reconfirmation` | A deleted or edited endpoint needs a new confirmation | Pass |

Stable over 6 runs (≈ 0.2 s each). **S5 passed on the first choice; the socket-level fallback was not needed. K8 retired.**

### Rules the gate applies (M5a)

- Address classes exactly as US-09.7 AC4: loopback → This machine; 10/8, 172.16/12, 192.168/16, fc00::/7 → Local network; everything else → Internet, including link-local and `100.64/10` (CGNAT, for example Tailscale). An IPv4-mapped IPv6 address is classified as its IPv4. `0.0.0.0` and `::` are refused.
- All addresses of a host are resolved; the most exposed class wins (INV-EG-03).
- A permission holds only for the origin (`scheme://host:port`) and the class confirmed. Any change of either needs a new confirmation, not only a move to a more exposed class (INV-EG-04, which wins over ADR-0009's wording). So does an edited or deleted endpoint.
- Every call runs lookup → resolve → check → read key → record → publish → pinned send. The record is written before the send (INV-EG-05); a refused call records nothing.
- The endpoint test sends only fixed texts and takes no project (INV-EG-08). It is not recorded as project egress.
- ASSUMPTION: plain `http` to an internet host is allowed; the confirm dialog says "Not encrypted (http)".
- ASSUMPTION: the base URL gets `/v1` only when it has no path; a typed path is kept, since some providers put the API elsewhere.

## 2. External path and endpoint suite (NFR-COMP-05)

Moved from M1. llama-server run on RM-1 (2026-11-03). **Ollama, LM Studio and calls from RM-2 need the maintainer:** Ollama and LM Studio are not installed on RM-1, and RM-2 is another machine.

The suite is `mangkon-cli endpoint test`. It sends the app's fixed test texts (a chat call, a structured-output probe with a JSON schema the prompt does not mention, and an embeddings call), prints where the host points, the model it reports and the times, and saves nothing. The key is read from an environment variable and never printed.

```sh
# On RM-1 (loopback) and from RM-2 (over the LAN, with RM-1's address):
mangkon-cli endpoint test http://127.0.0.1:8080 --model <name> --json        # llama-server
mangkon-cli endpoint test http://127.0.0.1:11434 --model <name> --json       # Ollama
mangkon-cli endpoint test http://127.0.0.1:1234 --model <name> --json        # LM Studio
MK_KEY=... mangkon-cli endpoint test https://<provider>/v1 --model <name> --api-key-env MK_KEY
```

| Runtime | Version | From | Chat | Structured output | Embeddings | Notes |
|---|---|---|---|---|---|---|
| llama-server | build 10863 | RM-1 (loopback) | ok | supported | ok, 1024 dimensions | Two servers, one per role. Chat: `Qwen_Qwen3.5-4B-Q4_K_M`, `--api-key`; embeddings answered 501 with llama-server's hint "Start it with `--embeddings`". Embeddings: `Qwen3-Embedding-0.6B-Q8_0` with `--embeddings`. Wrong key and no key → `key_refused`, exit 1; a port with nothing listening → `cannot_reach` |
| llama-server | build 10863 | RM-1 (own LAN address 192.168.1.x) | ok | supported | — (chat server) | Classified Local network; the key was required |
| llama-server | — | RM-2 (LAN) | — | — | — | |
| Ollama | — | RM-1 | — | — | — | |
| Ollama | — | RM-2 (LAN) | — | — | — | |
| LM Studio | — | RM-1 | — | — | — | |
| LM Studio | — | RM-2 (LAN) | — | — | — | |

Found by the real run: a thinking model (Qwen3.5-4B) used 222 tokens on reasoning before the reply. With the probe's old 64-token limit the reply came back empty (`finish_reason: length`), so structured output was wrongly reported as not supported. The probe now allows 1024 tokens; the plain chat probe still uses 16, since any well-formed reply passes it. llama-server does not check the model name (one model per server), so "Model '…' not found" cannot appear there.

The fake-server part of the suite runs in CI: `crates/mk-core/tests/m5a_egress.rs` (`us_09_6_ac2_*`, `us_09_6_ac3_*`) and `apps/cli/tests/endpoint.rs` cover success, a refused key, a missing model, nothing listening, an unknown host, no OpenAI-compatible API, structured output missing, and embeddings missing.

## 3. Keys never on disk (NFR-SEC-07, ADR-0012)

| Check | How | Result |
|---|---|---|
| Fakes, every CI run | `nfr_sec_07_test_key_never_on_disk`: a known key through add, test (twice, once refused with the key echoed in the server's error body) and two gate calls; every file under the temporary app-data and project folders is searched for it, with the databases open (WAL and SHM present) and after closing; the shown endpoint, test, permission and record values are searched too | Pass |
| CLI output | `apps/cli/tests/endpoint.rs`: stdout and stderr of `endpoint test`, text and JSON, with a right and a wrong key | Pass |
| Real keychain (GNOME Keyring, RM-1) | `nfr_sec_07_real_keychain` (ignored in CI): the key goes to the Secret Service under `mangkon-ai` / `endpoint:<id>`, a test and a gate call send it, no file holds it, a second workspace finds it in the keychain, and after delete the keychain has no entry | Pass |
| No Secret Service | The same test with `DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent MK_EXPECT_SESSION_ONLY=1`: session-only store with the notice, no file holds the key, a second workspace shows the key as missing | Pass |
| Release app, webview data folder | By hand: add an endpoint with a known key in the release app, test it, quit, then search the app-data folder, the project folder and the webview data folder (`~/.local/share/<identifier>/`) for the key | **Deferred** (maintainer's call, 2026-11-03) |

There is no log file yet (architecture §8), so "logs" is checked as stdout and stderr.

```sh
cargo test -p mk-core --test m5a_egress nfr_sec_07_real_keychain -- --ignored --nocapture
DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent MK_EXPECT_SESSION_ONLY=1 \
  cargo test -p mk-core --test m5a_egress nfr_sec_07_real_keychain -- --ignored --nocapture
```
