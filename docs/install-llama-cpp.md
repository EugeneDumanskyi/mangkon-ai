# Install llama.cpp (`llama-server`)

> Input: ADR-0015 (user-installed `llama-server`), `09-model-check.md` §4 (runtime check on RM-1). **If this file disagrees with them, they win.**
> Status: Written in M1 (2026-10-24); §2, §4 and §5 updated in M5c (2026-11-04). Linux checked on RM-1. macOS not verified here; Windows untested.
> Package versions were read from the package indexes on 2026-10-24. They change often; the build check below is what counts.

Mangkon AI runs local models through `llama-server` from [llama.cpp](https://github.com/ggml-org/llama.cpp). The app does not ship it and never downloads it. You install it once, and the app finds it. Layer 1 (identifiers, connections, keyword search) works without it. You can also use any OpenAI-compatible endpoint instead (`02-requirements.md` E09).

---

## 1. What the app needs

- **`llama-server` build 10863 or newer.** Release `v0.4.1` (build 10964) or newer qualifies. `v0.4.0` (build 10809) is too old.
- The app looks in two places, in this order: the path set in the app settings, then `llama-server` on your `PATH` (ADR-0015). It searches nowhere else.
- If it finds none, AI work is paused with "llama-server not found". If the build is too old, it says "llama.cpp build `<n>` found; build 10863 or newer is needed". Both reasons link here. After you install or update, choose "Retry"; no app restart is needed.

Check your build:

```sh
llama-server --version
```

The output has a line like this one (from RM-1):

```text
version: 0.4.0-dev (build 10863, commit 88ada91c1)
```

The number after `build` must be 10863 or higher. Older builds print `version: <build> (<commit>)`; the app reads both forms.

---

## 2. Pick a build for your GPU

| Your machine | Pick |
|---|---|
| NVIDIA GPU | A **CUDA** build. Prebuilt CUDA archives need the CUDA runtime: install it, or also download the matching `cudart-…` archive (440–594 MB on Linux). **Vulkan** is the smaller option (35 MB) and also works on NVIDIA, usually somewhat slower. |
| AMD or Intel GPU | A **Vulkan** build (ROCm for AMD and SYCL for Intel also exist). |
| Apple silicon | The macOS arm64 build or Homebrew. Metal is built in. |
| No GPU | The CPU build. It works, but extraction is much slower (see `09-model-check.md` §5). |

A CPU-only build on a GPU machine is slow, not broken. The AI models screen shows the path, build and backend it found (from `llama-server --list-devices`), and says which build to get when the machine has a GPU but the build runs on the CPU only. From the command line:

```sh
mangkon-cli runtime                     # or: --llama-server /path/to/llama-server
```

---

## 3. Install

### Prebuilt archive (every OS)

1. Open the [llama.cpp releases page](https://github.com/ggml-org/llama.cpp/releases). Every build `bNNNNN` is marked *pre-release*; the newest `vX.Y.Z` release names the build it stands for in `nightly-tag.txt`.
2. Download the archive for your OS and backend, for example `llama-<build>-bin-ubuntu-vulkan-x64.tar.gz`, `llama-<build>-bin-ubuntu-cuda-12.8-x64.tar.gz` (+ `cudart-…`), `llama-<build>-bin-macos-arm64.tar.gz`, or `llama-<build>-bin-win-vulkan-x64.zip`. The release page lists a SHA-256 for every file; compare it with `sha256sum <file>` (`shasum -a 256` on macOS, `certutil -hashfile <file> SHA256` on Windows).
3. Unpack it to a folder of your choice.
4. Put that folder on your `PATH`, or set the full path to `llama-server` in the app settings.

The full asset list per OS and backend (build `b11829`, 2026-10-23) is in `08-engine-bakeoff.md` §16.

### Package managers

| OS | Command | Version on 2026-10-24 | Enough? |
|---|---|---|---|
| macOS, Linux | `brew install llama.cpp` (Homebrew) | 0.7.0 = build 11669 | Yes |
| macOS | `sudo port install llama.cpp` (MacPorts) | 0.7.0 | Yes |
| Windows | `winget install ggml.llamacpp` | build 11829 | Yes |
| Windows | `scoop install llama.cpp-vulkan` (also `-cpu`, `-cu124`, `-sycl`; `versions` bucket) | build 11840 | Yes |
| Debian unstable / 14 | `apt install llama.cpp` | 0.7.0 | Yes |
| Arch Linux | `pacman -S llama-cpp` (`extra`) | 0.6.0 = build 11429 | Yes |
| openSUSE Tumbleweed | `zypper install llamacpp` | 0.7.0 | Yes |
| Fedora rawhide | `dnf install llama-cpp` | build 11232 | Yes |
| Nix (unstable) | `llama-cpp`, `llama-cpp-cuda`, `llama-cpp-vulkan`, `llama-cpp-rocm` | 0.6.0 | Yes |
| **Ubuntu 26.04 / 26.10** | `apt install llama.cpp` | build 8681 / 10438 | **No.** Use a prebuilt archive or Homebrew. |
| **Fedora 44** | `dnf install llama-cpp` | build 6153 | **No.** |
| **Nix 26.05 (stable)** | `llama-cpp` | build 9190 | **No.** Use `nixpkgs-unstable`. |

Distribution packages often build only the CPU or Vulkan backend. Run `llama-server --version` after installing; with a GPU, check that the startup log names it (`CUDA`, `Vulkan` or `Metal`).

### From source

Possible on every OS with CMake and a C++ compiler; RM-1 uses a CUDA source build:

```sh
git clone https://github.com/ggml-org/llama.cpp && cd llama.cpp
cmake -B build -DGGML_CUDA=ON        # or -DGGML_VULKAN=ON, or nothing for CPU
cmake --build build --config Release -j
build/bin/llama-server --version
```

---

## 4. How the app runs it

You do not start `llama-server` yourself; the app starts its own process for each loaded model. It runs:

```text
# extraction model
llama-server --host 127.0.0.1 --port <free port> --api-key <random per start> -m <model> -c <context> -np 1 --cache-ram 0 --reasoning off
# embedding model
llama-server --host 127.0.0.1 --port <free port> --api-key <random per start> -m <model> --embedding -c 2048 -np 1 -b 2048 -ub 2048 --cache-ram 0
```

- It starts when AI work (or "Try" on the AI models screen) needs the model, and stops after 10 minutes without a call, when you choose another model, and when the app quits.
- It listens on `127.0.0.1` only, on a new random port with a new random key at every start (NFR-SEC-06). Other machines cannot reach it.
- If it cannot start or keeps stopping, AI work waits with the reason ("Not enough GPU memory", "The model file could not be loaded", …) and everything else goes on. After a crash it is started again at most twice. "Show details" on the AI models screen has its last output; "Retry" starts it again.
- A request without the key gets HTTP 401; only `/health` answers without one.
- Models come from Hugging Face, checked by SHA-256 against the app's catalog (NFR-SEC-11). The app downloads models, never programs.

---

## 5. Tested

| OS | Build | How | Result |
|---|---|---|---|
| Linux x86_64 (RM-1, Ubuntu-based, RTX 5070 Laptop) | 10863 (`88ada91c1`), CUDA source build | `model-check runtime`, `09-model-check.md` §4 | Found, version read, started, health-checked, killed and restarted |
| Linux x86_64 (RM-1) | 10863 (`88ada91c1`), CUDA 13 source build, driver 595.104.02 | `mangkon-cli runtime`, `13-model-manager.md` §5 (M5c) | Found by settings path and on `PATH`, build and CUDA backend read. Started by the app with the 4B model (Try, CLI and desktop), 127.0.0.1 only; a 35B Q8_0 model starts with the default fit (6588 MiB on the GPU, the rest in RAM) |
| macOS | — | Not verified here | — |
| Windows | — | Untested | — |
