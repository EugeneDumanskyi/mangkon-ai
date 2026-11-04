# 13 — Model manager and egress gate

> Input: `06-roadmap.md` §2 M5 (spike S5), ADR-0009, ADR-0012, ADR-0015, `02-requirements.md` E09, `03-nfr.md` (NFR-SEC-02/04/05/07, NFR-COMP-05), `04-domain.md` BC-11 and §7.6. **If this file disagrees with them, they win.**
> Status: M5a done on RM-1 (2026-11-03). §2: llama-server done; Ollama and LM Studio dropped (2026-11-04); calls from RM-2 moved to M10. §3: the release-app webview check moved to M10. §4 (M5b): code and automated checks done on RM-1 (2026-11-04); resume after a killed download checked on the real CDN; the network capture was not done (skipped, 2026-11-04; moved to M10). §5–§6 (M5c): done on RM-1 (2026-11-04): automated checks, Try with the 4B model, the out-of-memory text, the desktop walk. Linux only; macOS and Windows are not verified here.
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
- Plain `http` to an internet host is allowed; the confirm dialog says "Not encrypted (http)". Confirmed by the maintainer (2026-11-03).
- The base URL gets `/v1` only when it has no path; a typed path is kept, since some providers put the API elsewhere. Confirmed by the maintainer (2026-11-03).
- The endpoint test probes chat and embeddings and shows which work; an endpoint's role is chosen in M6. Confirmed by the maintainer (2026-11-03).
- Link-local and `100.64/10` count as Internet, as US-09.7 AC4 says. Confirmed by the maintainer (2026-11-03).

## 2. External path and endpoint suite (NFR-COMP-05)

Moved from M1. llama-server run on RM-1 (2026-11-03). For M5a, the run against RM-1's own LAN address stands in for the Local network path. Calls from RM-2 moved to M10; Ollama and LM Studio dropped from the test list (maintainer's calls, 2026-11-03 and 2026-11-04): they are OpenAI-compatible endpoints like any other, and the app does not depend on them.

The suite is `mangkon-cli endpoint test`. It sends the app's fixed test texts (a chat call, a structured-output probe with a JSON schema the prompt does not mention, and an embeddings call), prints where the host points, the model it reports and the times, and saves nothing. The key is read from an environment variable and never printed.

```sh
# On RM-1 (loopback) and from RM-2 (over the LAN, with RM-1's address):
mangkon-cli endpoint test http://127.0.0.1:8080 --model <name> --json        # llama-server
MK_KEY=... mangkon-cli endpoint test https://<provider>/v1 --model <name> --api-key-env MK_KEY
```

| Runtime | Version | From | Chat | Structured output | Embeddings | Notes |
|---|---|---|---|---|---|---|
| llama-server | build 10863 | RM-1 (loopback) | ok | supported | ok, 1024 dimensions | Two servers, one per role. Chat: `Qwen_Qwen3.5-4B-Q4_K_M`, `--api-key`; embeddings answered 501 with llama-server's hint "Start it with `--embeddings`". Embeddings: `Qwen3-Embedding-0.6B-Q8_0` with `--embeddings`. Wrong key and no key → `key_refused`, exit 1; a port with nothing listening → `cannot_reach` |
| llama-server | build 10863 | RM-1 (own LAN address 192.168.1.x) | ok | supported | — (chat server) | Classified Local network; the key was required |
| llama-server | — | RM-2 (LAN) | — | — | — | M10 |

Found by the real run: a thinking model (Qwen3.5-4B) used 222 tokens on reasoning before the reply. With the probe's old 64-token limit the reply came back empty (`finish_reason: length`), so structured output was wrongly reported as not supported. The probe now allows 1024 tokens; the plain chat probe still uses 16, since any well-formed reply passes it. llama-server does not check the model name (one model per server), so "Model '…' not found" cannot appear there.

The fake-server part of the suite runs in CI: `crates/mk-core/tests/m5a_egress.rs` (`us_09_6_ac2_*`, `us_09_6_ac3_*`) and `apps/cli/tests/endpoint.rs` cover success, a refused key, a missing model, nothing listening, an unknown host, no OpenAI-compatible API, structured output missing, and embeddings missing.

## 3. Keys never on disk (NFR-SEC-07, ADR-0012)

| Check | How | Result |
|---|---|---|
| Fakes, every CI run | `nfr_sec_07_test_key_never_on_disk`: a known key through add, test (twice, once refused with the key echoed in the server's error body) and two gate calls; every file under the temporary app-data and project folders is searched for it, with the databases open (WAL and SHM present) and after closing; the shown endpoint, test, permission and record values are searched too | Pass |
| CLI output | `apps/cli/tests/endpoint.rs`: stdout and stderr of `endpoint test`, text and JSON, with a right and a wrong key | Pass |
| Real keychain (GNOME Keyring, RM-1) | `nfr_sec_07_real_keychain` (ignored in CI): the key goes to the Secret Service under `mangkon-ai` / `endpoint:<id>`, a test and a gate call send it, no file holds it, a second workspace finds it in the keychain, and after delete the keychain has no entry | Pass |
| No Secret Service | The same test with `DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent MK_EXPECT_SESSION_ONLY=1`: session-only store with the notice, no file holds the key, a second workspace shows the key as missing | Pass |
| Release app, webview data folder | By hand: add an endpoint with a known key in the release app, test it, quit, then search the app-data folder, the project folder and the webview data folder (`~/.local/share/<identifier>/`) for the key | **Moved to M10** (maintainer's calls, 2026-11-03 and 2026-11-04) |

There is no log file yet (architecture §8), so "logs" is checked as stdout and stderr.

```sh
cargo test -p mk-core --test m5a_egress nfr_sec_07_real_keychain -- --ignored --nocapture
DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent MK_EXPECT_SESSION_ONLY=1 \
  cargo test -p mk-core --test m5a_egress nfr_sec_07_real_keychain -- --ignored --nocapture
```

---

## 4. Hardware check and downloads (M5b)

### Hardware check on RM-1

`mangkon-cli hardware` (2026-11-04):

| Part | Found |
|---|---|
| OS | Linux (Pop!_OS 24.04) |
| CPU | Intel Core i7-14650HX (24 threads) |
| RAM | 101 GB |
| GPU (used) | NVIDIA GeForce RTX 5070 Laptop GPU, VRAM 8.5 GB (8151 MiB), driver 595.104.02 |
| GPU | Intel Raptor Lake-S UHD Graphics, VRAM unknown (integrated) |
| Verdict | Meets minimum, tier `large-ram` |

GB is 10^9 bytes (decided 2026-11-04): the 8 GB card reports 8151 MiB = 8.55 × 10^9 bytes, which would be below 8 GiB. The probe reads `nvidia-smi`, `/sys/class/drm` and `lspci` and takes about 2.5 s; it runs once per session and again on "Check hardware". The macOS (`sysctl`, `system_profiler`) and Windows (PowerShell) probes are written but not verified here.

### Download hosts

The catalog lists the hosts a download may reach (`download_hosts`): `huggingface.co`, `*.cdn.hf.co`, `cdn-lfs.hf.co`, `cdn-lfs-us-1.hf.co`, `cdn-lfs-eu-1.hf.co`, `cas-bridge.xethub.hf.co`. Every hop must be HTTPS to a listed host; redirects are followed by the app (at most 5) and an unlisted host is refused before its name is looked up.

Seen on 2026-11-04: `https://huggingface.co/<repo>/resolve/<revision>/<file>` answers 302 to a signed URL on `us.aws.cdn.hf.co` (`/xet-bridge-us/…`), which accepts `Range`. The host has two labels below `cdn.hf.co`, so `*.` matches any name below the suffix, not one label (deviation from the plan, which said one label; the names stay Hugging Face's).

### Real downloads (RM-1)

| Run | Result |
|---|---|
| `nomic-embed-text-v1.5-q8_0` (146 MB) through the download module, paused after 2.7 s, then resumed | Redirect to `us.aws.cdn.hf.co`; paused at 59,083,193 bytes; the resume re-requested the `resolve` URL and sent `Range: bytes=59083193-` to the new signed URL; 206 with the right `Content-Range`; SHA-256 matched; 5.4 s in all (about 32 MB/s) |
| `mangkon-cli model download nomic-embed-text-v1.5-q8_0`, then `model delete` | Installed (about 32 MB/s), listed as installed, deleted, folder gone |
| `mangkon-cli model download qwen3.5-4b-q4_k_m` (2.9 GB) killed (SIGKILL) after 20 s, then run again | Killed at 616,039,879 bytes, listed as Interrupted 0.6/2.9 GB with the `.part` kept; the second run started at 616 MB (not 0), finished at about 20–34 MB/s, SHA-256 matched, installed (2,871,743,520 bytes) |

The kill-and-rerun replaces the planned network-off run (decided 2026-11-04): turning the network off cuts the whole machine. It checks resume on the real CDN (a fresh signed URL and `Range` from the `.part`). Losing the connection while the app keeps running is checked with the fakes only (cut body, 30 s stall limit).

```sh
mangkon-cli model download qwen3.5-4b-q4_k_m &   # kill -9 it after about 20 s
mangkon-cli models                               # interrupted 0.6/2.9 GB
mangkon-cli model download qwen3.5-4b-q4_k_m     # resumes, verifies, installs
```

### Automated checks (fakes, every CI run)

A fake Hugging Face (`mk-models` feature `testkit`) serves byte ranges over plain HTTP, redirects to a second host name, and can cut a body, send wrong bytes, ignore `Range`, answer an error status or go slowly; a resolver knows only the test names and records every lookup.

| Requirement | Tests | Result |
|---|---|---|
| US-09.1 AC2–AC4, NFR-COMP-04 | `mk-models` `hardware` unit tests (`us_09_1_ac2_*`, `us_09_1_ac3_vulkan_best_effort`, `us_09_1_ac3_best_gpu_decides`, `us_09_1_ac4_apple_24gb`, `nfr_comp_04_old_driver`) | Pass |
| US-09.2 AC1–AC3 | `crates/mk-core/tests/m5b_models.rs` (`us_09_2_ac1_no_local_download_below_minimum`: refused with the AC1 text, nothing requested, no name looked up; `us_09_2_ac2_layer_1_works_below_minimum`; `us_09_2_ac3_override_enables`), `ui/src/models/hardware.test.tsx` | Pass |
| US-09.3 AC1–AC3 | `us_09_3_ac1_recommended_per_tier`, `us_09_3_ac2_default_workspace_uses_the_shipped_catalog`, `nfr_mnt_09_valid_catalog_passes` (the shipped file validates), `ui/src/models/catalog.test.tsx` (license before Download) | Pass |
| US-09.4 AC1, NFR-RES-05 | `us_09_4_ac1_disk_check_blocks`, `nfr_res_05_disk_check_before_download` (free space read again at the start); a write that hits a full disk ends Interrupted with "Disk full" | Pass |
| US-09.4 AC2 | `us_09_4_ac2_progress_pause_cancel` (progress, speed, time left, one download at a time, pause keeps the partial file, cancel deletes it) | Pass |
| US-09.4 AC3, NFR-REL-08 | `us_09_4_ac3_resume_after_drop` (resume asks for the rest only), `us_09_4_ac3_resume_after_restart` (a download running at quit is Interrupted after restart, its partial file kept); `mk-models` `tests/downloads.rs` (drops, a server that ignores `Range`, split models) | Pass |
| US-09.4 AC4, NFR-SEC-11 | `us_09_4_ac4_checksum_mismatch_deletes`, `nfr_sec_11_tampered_partial_is_rejected` (a partial file changed while paused); no path reaches Installed without a matching SHA-256 (`domain_7_7_transitions`) | Pass |
| US-09.4 AC5 | `us_09_4_ac5_delete_frees_space` (folder gone, free space read again) | Pass |
| NFR-SEC-02 | `nfr_sec_02_redirect_outside_catalog_refused` and the `mk-models` host tests: an unlisted or non-HTTPS hop is refused and never looked up | Pass |

### Network capture (acceptance criterion 7)

**Not done** (skipped by the maintainer, 2026-11-04; it needs sudo; moved to M10). Criterion 7 rests on the automated host checks above. To run it later: during a real model download and `mangkon-cli endpoint test` against RM-1's llama-server, capture all traffic, then list the DNS names and TLS SNI. Expected: only `huggingface.co`, names under `cdn.hf.co` and the endpoint.

```sh
sudo tcpdump -i any -w /tmp/m5b.pcap &
mangkon-cli model download nomic-embed-text-v1.5-q8_0 --app-data /tmp/mk-app
mangkon-cli endpoint test http://127.0.0.1:8080 --model local
sudo kill %1
tshark -r /tmp/m5b.pcap -Y 'dns.flags.response == 0' -T fields -e dns.qry.name | sort -u
tshark -r /tmp/m5b.pcap -Y 'tls.handshake.type == 1' -T fields -e tls.handshake.extensions_server_name | sort -u
```

### Rules and assumptions (M5b)

- One download at a time (decided 2026-11-04; a queue can come later). State is kept in `app.db` (v3: `model_download`, `model_file`, `app_setting`) on every change of state, and the byte count about once a second. A download running when the app quits is Interrupted on the next start (domain §7.7), and resumes from its partial file.
- Start guard (INV-MM-02, INV-MM-03): GPU and memory meet the minimum or the override is on; free disk ≥ the bytes still to download + 2 GB (decided 2026-11-04: remaining, not the whole model, so a resume near the end is not refused). The 30 GB of US-09.1 AC2 is a note in the hardware check, not part of the verdict or the start guard (decided 2026-11-04); a model whose download does not fit shows "Does not fit: needs N GB free disk" on its row.
- `*.cdn.hf.co` matches any name below `cdn.hf.co` (decided 2026-11-04): Hugging Face controls that zone, and every file is checked against the catalog's SHA-256.
- Resume re-requests the `resolve` URL (signed CDN links expire) and needs a 206 with exactly the expected `Content-Range`; anything else restarts the file from 0. The bytes already on disk are hashed before the rest arrives.
- A stalled connection gives up after 30 s with no data and leaves the model Interrupted.
- NVIDIA driver older than 525.60 (Linux) or 527.41 (Windows) gets a note to update (CUDA 12.0 minimums from NVIDIA's release notes). It never blocks. Checked in M5c (2026-11-04): RM-1's `llama-server` is a source build against the CUDA 13 runtime (`libcudart.so.13`) with driver 595.104.02, and the app shows its backend from `--list-devices` (§5). ASSUMPTION: the prebuilt `cuda-12.8` archive works from driver 525.60 through CUDA minor-version compatibility, and CUDA 13 builds need driver 580 or newer. Not tested; the note stays a note.


---

## 5. Local runtime (M5c, US-09.5, ADR-0015)

### Finding and checking `llama-server`

- Where: the path in settings (`app_setting` `llama_server_path`, "Choose…" on the AI models screen), then `PATH`. Nowhere else. A settings path that is not an executable file is refused when it is set.
- Build: `--version` (both forms of the output), at least 10863; refusal texts exactly as ADR-0015, with `docs/install-llama-cpp.md`.
- Backend: `--list-devices`. Each device line starts with its backend's name: `CUDA0: NVIDIA GeForce RTX 5070 Laptop GPU (7707 MiB, 7570 MiB free)` on RM-1 (build 10863). No device line means a CPU-only build; with a GPU in the hardware check the screen then says which build to get. ASSUMPTION: the prefixes `Vulkan`, `MTL`/`Metal`, `ROCm`/`HIP` and `SYCL` for the other backends (read from the llama.cpp sources, not run).
- Both calls have a 5 s limit. "Check again" runs them again; "Retry" also starts failed models again. No app restart is needed.

```sh
mangkon-cli runtime --llama-server ~/AI/llama.cpp/build/bin/llama-server
```

```text
llama-server  ~/AI/llama.cpp/build/bin/llama-server (--llama-server)
Build         10863 (commit 88ada91c1, 0.4.0-dev)
Device        CUDA0: NVIDIA GeForce RTX 5070 Laptop GPU (7707 MiB, 7570 MiB free)
Backend       CUDA
Verdict       usable (build 10863 or newer)
```

### How it is started

```text
llama-server --host 127.0.0.1 --port <free port> --api-key <64 hex, new per start> -m <first file>
  extraction: -c <catalog context> -np 1 --cache-ram 0 --reasoning off
  embeddings: --embedding -c 2048 -np 1 -b 2048 -ub 2048 --cache-ram 0
```

- One process per loaded model, at most two (extraction and embeddings, architecture §7). Started when an AI stage or Try needs it (`Workspace::ai_client`, `try_local_model`), Ready only after `/health` answers 200 (polled every 250 ms, 300 s limit for large models).
- A lease holds it Busy during a call; it goes back to Ready after. A successful call sets the restart count back to 0 (INV-MM-06).
- Stopped after 10 min with no call (NFR-RES-06), when its role switches to another model or endpoint, and on app quit: SIGTERM, then kill after 10 s, and the app waits for it (US-09.5 AC2). A start still loading at quit is stopped too.
- The app's own malloc tunables are not passed on: `GLIBC_TUNABLES` is set back to the user's own value.
- Output (stdout and stderr) is kept as the last 16 KiB in memory only, for "Show details". No log file.
- Each call goes through the egress gate as "This machine" with the per-start key as Bearer token (INV-EG-04/05), so it is shown in the status bar and recorded like any other destination.

### Failures (US-09.5 AC3, NFR-REL-12)

| What happens | Shown | Source of the text |
|---|---|---|
| Not found / build too old | "llama-server not found" / "llama.cpp build N found; build 10863 or newer is needed" | ADR-0015 |
| Out of GPU memory at load | "Not enough GPU memory" | RM-1, build 10863 (35B, `--fit off -ngl 99`): `cudaMalloc failed: out of memory`, `failed to allocate CUDA0 buffer`, then `error loading model … failed to load model`, exit 1. Memory is checked before the model-file text, so this wins. ASSUMPTION: Vulkan prints `ErrorOutOfDeviceMemory` (not run) |
| Bad or missing model file | "The model file could not be loaded" | RM-1, build 10863 (the first 16 MB of the 4B file): `error loading model: tensor 'token_embd.weight' data is not within the file bounds, model is corrupted or incomplete`, `failed to load model`, exit 1 |
| Port taken between the check and the bind | Tried once more on a new port | RM-1, build 10863: `couldn't bind HTTP server socket, hostname: 127.0.0.1, port: N`, before the model loads |
| No `/health` in 300 s | "llama-server did not get ready within 300 s" | — |
| Anything else | "llama-server stopped (exit N)" / "(signal N)" plus Show details | — |
| Crash while Ready or Busy | Started again, at most twice; then Failed: "llama-server stopped again after 2 restarts (signal 9)" | INV-MM-06 |

When the role's model cannot run, the project's AI jobs (stages B and C for extraction, Embeddings for embeddings) are Blocked with that reason (INV-PR-09), the progress panel and the status bar show it, and Stage A goes on. They run again when the reason is gone: Retry, Choose…, allowing an endpoint, installing a model.

### Automated checks (fakes, every CI run)

`mk-core-fake-llama-server` / `mk-fake-llama-server` (feature `testkit`) answers `--version`, `--list-devices`, `/health`, chat and embeddings with the key check of the real one, and fails on request (slow start, out of memory, bad model, exit, crash after N ms, port taken once). The real process path runs against it:

- `crates/mk-models/tests/runtime.rs` (13 tests): start on demand on 127.0.0.1 with key (401 without, 200 with) and a `/proc/net/tcp` check that it listens on loopback only; a new port and key per start; one start for concurrent callers; idle stop; quit while Ready and while still loading; each failure text; not found then set in settings; port taken once; health timeout; crash → 2 restarts → Failed; a success resets the count; model switch; `kill -9`.
- `crates/mk-core/tests/m5c_runtime.rs` (13 tests): the ADR-0015 view; `ai_client` and Try for both roles; quit; idle; out of memory → AI job Blocked with the reason while Stage A of three files finishes, then Retry unblocks it; `kill -9` three times → Blocked, Stage A goes on; "No model allowed for this project" with no model, with an endpoint not allowed, and again after revoke; the default choice and a switch that stops the model; NFR-SEC-05 for the three classes (below); first run and demo project (§6).
- `ui/src/status/statusBar.test.tsx`, `ui/src/models/runtime.test.tsx`.

NFR-SEC-05 (destination during every AI call): the gate publishes each call before it is sent and clears it when it ends, success or error. The test sends through `ai_client` to hosts that resolve to `127.0.0.1` plus a LAN or an internet address (the most exposed class wins, the send goes to the first checked address), so all three classes are shown without a packet leaving the machine. Try and the endpoint test publish the same way.

### Real runs on RM-1 (2026-11-04)

Machine idle (GPU 2 of 8151 MiB used, load 0.6), build 10863 CUDA, `qwen3.5-4b-q4_k_m` from §4.

```sh
mangkon-cli model try qwen3.5-4b-q4_k_m --llama-server ~/AI/llama.cpp/build/bin/llama-server
```

```text
Started   127.0.0.1:33493 in 1467 ms (pid 160382)
Listening 127.0.0.1:33493
Result    Connected: …/Qwen_Qwen3.5-4B-Q4_K_M.gguf answered in 173 ms
Stopped
```

- llama-server reports the model file path as the model's name, so the result line showed the path. Since then Try names the catalog model ("Connected: Qwen3.5 4B answered in … ms"); `TestReport::summary_as`.
- Exit 0, 2.8 s in all. `ss -ltnp` during the run: `127.0.0.1:33493` only, owned by that pid. No `llama-server` left after.
- `--json`, run again: another port (45687), started in 1459 ms, answered in 136 ms, structured output supported.
- Arguments as started: `--host 127.0.0.1 --port <P> --api-key <key> -m …/Qwen_Qwen3.5-4B-Q4_K_M.gguf -c 8192 -np 1 --cache-ram 0 --reasoning off`.
- Out of memory: `Qwen3.6-35B-A3B-Q8_0` (35205 MiB on an 8 GiB GPU) with `--fit off -ngl 99` failed after about 33 s with the text in the table above. With the app's own arguments (llama.cpp's default fit, no `-ngl`) the same model was ready in 5 s, 6588 MiB on the GPU and the rest in RAM, and stopped with exit 0 on SIGTERM. So with the app's arguments a model too large for the GPU still starts; "Not enough GPU memory" is for a GPU that is already busy or a fit that fails.
- A cut model file (the first 16 MB of the 4B file): "The model file could not be loaded", text in the table above.

Desktop walk (release build, `cargo xtask build --release`):

| Step | Result |
|---|---|
| First run, fresh app data folder (`XDG_DATA_HOME` set to an empty folder) | "Step 1 of 3" with the hardware check; step 2 with the recommended models and "Skip — use without AI"; step 3 with the demo folder found next to the binary (`testdata/td1`). "Open demo project" created `<app data>/demo-project` and read its files; the status bar showed "No model allowed for this project · Settings", and the link opened the project's Settings tab |
| Start again, same folder | No first-run screen; the start screen with the demo project in the list |
| Quit | No `mangkon` or `llama-server` process left |
| Normal app data, `llama-server` on `PATH` | The llama-server section showed the path (found on PATH), build 10863 and CUDA. Try on the 4B model: "Data sent to this machine" during the call, then "People and organizations: Ready · 127.0.0.1:40239"; `ss -ltnp`: 127.0.0.1 only, the arguments as above |
| `kill -9` three times | Started again on a new port each time (36721, then 33833), listening about 1–1.5 s after the kill; the third kill left it Failed with Retry, and no new process came up in 20 s. Retry: Starting… then Ready |
| 10 min with no call | After Retry it went Ready; with no call `llama-server` stopped by itself about 10 min later (watched for 572 s from just after Ready), and the GPU memory was freed |
| Quit while Ready | Try, then the window closed: no `mangkon` or `llama-server` process left, no listening socket, GPU back to 2 MiB |

---

## 6. First run and demo project (M5c, US-14.4)

- Shown while `app_setting` `first_run_done` is unset and no project is in the list. Three steps, each skippable: This machine (the hardware check), AI model (the recommended models with Download, an endpoint, or "Skip — use without AI"; below the minimum only the endpoint or Skip), First project (New project, or the demo project). "Skip setup" ends it at any step.
- An endpoint added here is used for people and organizations; meaning search is set on the AI models screen.
- The demo files are TD-1 (`testdata/td1`, `07-test-corpus.md`). They come with the source code, not with the app, so the app looks for `testdata/td1/ground-truth.json` written by `corpus-gen` in the folders above the running binary and the current folder. When that fails (a binary copied elsewhere), "Choose the demo folder…" takes TD-1, `td1/`'s parent or the repository folder.
- The demo project is `<app data>/demo-project` ("Demo project", a number added if the name is taken) with TD-1's `synthetic/` and `public/` folders as sources; opening it again reuses it. Its files are read at once.
- Checked by `us_14_4_*` in `crates/mk-core/tests/m5c_runtime.rs` and `ui/src/first/firstRun.test.tsx`. The hand walk on a fresh app data folder passed on RM-1 (2026-11-04, §5).
