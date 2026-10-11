# LiteBox

A lightweight Android app runtime written in Rust with zero dependencies. It
runs real Android APKs headless so AI agents can install, launch, drive, pause,
inspect, and replay them. One binary, three control surfaces: CLI, JSON-RPC
over HTTP, and MCP.

**Source:** https://github.com/loak7993-code/litebox

LiteBox is not a full-system emulator. It does not boot an Android system
image, and it does not run native `.so` libraries or graphics-heavy UI. It runs
the DEX bytecode of real APKs against an emulated framework subset, which is
the layer where app logic, crashes, state, and lifecycle live. That makes it
small (one static binary), fast (18M VM instructions/s release), deterministic
(seeded RNG + virtual clock), and debuggable at bytecode granularity.

## What it runs

- Real APKs produced by the normal Android toolchain (javac/kotlin -> d8 ->
  apksigner). Multi-dex supported.
- App logic: activities' `onCreate`/`onResume`, business logic classes,
  collections, strings, threads, handler posts, exceptions.
- A framework subset: `java.lang` (String, StringBuilder, boxed types, Math,
  System, throwables), `java.util` (ArrayList, HashMap, HashSet, Arrays,
  Objects, Random), `android.*` basics (Log, Build, Context, Activity,
  View/TextView/Button/EditText/LinearLayout, Intent, Bundle, Handler).
- Anything outside that subset throws a catchable
  `litebox.UnsupportedApiException` naming the exact API, so an agent sees
  precisely what is missing instead of a mystery crash.

Native `.so` code, `android.webkit`, resource/layout inflation, and WebView
are not implemented. Coverage is introspectable at runtime via `api.coverage`.

## Agent features

| Capability | Tool |
|---|---|
| Install / list / uninstall APKs | `pm.install`, `pm.list`, `pm.uninstall` |
| Start / stop apps, drive lifecycle | `app.start`, `app.stop`, `run` |
| Pause on exception or unsupported API | `pause_on` config, automatic |
| Instruction stepping + breakpoints | `step`, `breakpoints.set` |
| Inspect tasks, frames, objects, statics | `state.tasks`, `state.frames`, `state.object`, `state.statics` |
| Patch fields mid-run | `object.write`, `static.write` |
| Call app methods directly | `call` |
| Screenshot the framebuffer | `screen.png` |
| Deterministic record/replay | `--seed` + snapshots |
| Snapshot the whole VM, restore, replay | `snapshot.save`, `snapshot.load` |
| Logcat-style event stream | `logcat`, WebSocket `/events` |

Determinism: app-visible time is virtual and only advances when the scheduler
does; randomness comes from a seeded SplitMix64. Two runs with the same seed
and the same inputs produce identical runs, which makes flaky bugs reproducible
and lets a crash be replayed from a snapshot repeatedly.

## Quick start

```sh
# build (Rust 1.75+, no other deps)
cargo build --release
# binary: target/release/litebox

# run the control plane
LITEBOX_HOME=~/.litebox target/release/litebox serve --addr 127.0.0.1:7830

# install and start an APK
curl -s localhost:7830/rpc -d '{"jsonrpc":"2.0","id":1,"method":"pm.install","params":{"path":"app.apk"}}'
curl -s localhost:7830/rpc -d '{"jsonrpc":"2.0","id":2,"method":"app.start","params":{"package":"com.example.app"}}'

# drive it
curl -s localhost:7830/rpc -d '{"jsonrpc":"2.0","id":3,"method":"run"}'
curl -s localhost:7830/rpc -d '{"jsonrpc":"2.0","id":4,"method":"logcat","params":{}}'
curl -s localhost:7830/rpc -d '{"jsonrpc":"2.0","id":5,"method":"screen.png","params":{"out":"/tmp/shot.png"}}'
```

MCP (for Claude and other MCP clients): add this to the MCP config:

```json
{ "litebox": { "command": "/path/to/litebox", "args": ["mcp"] } }
```

CLI verbs share the same dispatcher:

```sh
litebox install app.apk
litebox apps
litebox start com.example.app
litebox run
litebox logcat --follow
litebox screen /tmp/shot.png
litebox status
litebox debug            # interactive REPL over the tool surface
```

Every command above has been run successfully against the bundled smoke app.

## Building the smoke APK

The repo bundles a tiny app used by the test suite. It is built with the real
Android toolchain if you have an SDK:

```sh
examples/smoke/build.sh     # javac -> d8 -> aapt2 -> apksigner
```

Without an SDK, the test suite skips the toolchain-built fixture and the dex
parser cross-checks against any `.dex`/`.apk` you point it at.

## Driving real Android (adb passthrough)

The same control plane also drives real Android devices — the bundled
`android15` redroid container, hardware phones, the official emulator —
through adb tools that exec the SDK's adb binary directly (no shell
interpolation, hard timeouts, structured JSON results):

| Tool | Purpose |
|---|---|
| `adb.devices` / `adb.connect` | enumerate / attach devices |
| `adb.shell` | run any shell command (`getprop`, `dumpsys`, ...) |
| `adb.install` / `adb.uninstall` / `adb.packages` | package management |
| `adb.am_start` | launch an activity |
| `adb.input` | tap / swipe / text / keyevent |
| `adb.ui` | parsed UI hierarchy (nodes with text, class, bounds, clickable) |
| `adb.screencap` | PNG screenshot (file or base64) |
| `adb.logcat` | logcat dump with filter |

`LITEBOX_ADB` overrides the adb path (default: the host SDK at
`/opt/android-sdk/platform-tools/adb`, then PATH).

An agent session over one surface, both layers — inspect the hierarchy, type
into a real app, tap its buttons, capture the result:

```sh
curl -s localhost:7830/rpc -d '{"jsonrpc":"2.0","id":1,"method":"adb.ui","params":{}}'
curl -s localhost:7830/rpc -d '{"jsonrpc":"2.0","id":2,"method":"adb.input","params":{"kind":"tap","args":[360,652]}}'
curl -s localhost:7830/rpc -d '{"jsonrpc":"2.0","id":3,"method":"adb.screencap","params":{"out":"/tmp/after.png"}}'
```

## Configuration

| Env / flag | Purpose | Default |
|---|---|---|
| `LITEBOX_HOME` | Data directory (packages, snapshots) | `~/.litebox` |
| `LITEBOX_SEED` | Deterministic RNG seed | fixed |
| `--addr HOST:PORT` | HTTP listener for `serve` | `127.0.0.1:7830` |
| `pause_on` (config file) | `exception` / `exception_and_unsupported` | `exception` |
| `heap_budget_bytes` | GC trigger threshold | 256 MiB |
| `surface_width/height` | Framebuffer size | 720x1280 |

## Auth (for public exposure)

Set `LITEBOX_TOKEN` before `serve` and every route (RPC, MCP, WebSocket
events) requires `Authorization: Bearer <token>` or `X-Litebox-Token` —
verified from the public internet: 401 without, 200 with. Unset = open, for
local use. The flag also supports `--addr 0.0.0.0:7830` for public binds.
Plaintext HTTP caveat: run behind a tunnel (cloudflared) for TLS — account-less
quick tunnels returned edge-404s from this host's IP range during testing, so
a named tunnel on your own Cloudflare account is the TLS path.

## Operations & RAM profile

The Android container is capped at 1.5 GB (swap to 2 GB) and stripped for
headless automation: Bluetooth and the Latin IME are disabled via
`pm disable-user` (persisted in the data volume — adb text injection doesn't
need an IME). Helper scripts:

```sh
scripts/android-start.sh   # start + wait for boot (docker start or first run)
scripts/android-trim.sh    # drop caches, kill background procs, trim residents
scripts/android-stop.sh    # stop entirely: container RAM goes to ~0
```

Measured on this machine: Android 15 container ~1.0 GB steady-state (capped
1.5 GB), LiteBox server ~4 MB, binary 1.53 MB static.

## Vanilla profile

The android15 container runs **pure AOSP — zero Google/GMS packages** (verified
`pm list packages | grep -ci google` = 0) stripped to the basic set:

**Kept:** Launcher, SystemUI, Settings, Calendar, Camera, Clock, Contacts,
Files (DocumentsUI), Gallery, Music, WebView (library), plus your installed
apps. **Disabled:** telephony stack (no radio), NFC/SE/UWB, printing, ads
SDKs (AdServices, OnDevicePersonalization, FederatedCompute), HealthConnect,
daydreams, wallpapers pickers, musicfx, searchbox, MTP, CTS shims, Chromium
shell, Bluetooth, IME.

Apply / re-apply after any container recreate with `scripts/android-strip.sh`
(idempotent). Revert anything with
`adb shell pm enable --user 0 <package>`.

## Architecture

```
crates/
  litebox-json     JSON parser/writer (insertion-ordered, RFC 8259)
  litebox-zip      ZIP reader/writer + full DEFLATE inflate + CRC32
  litebox-dex      DEX 035-039 parser: strings (MUTF-8), protos, class data,
                   code items, try/catch tables, encoded static values
  litebox-core     Kernel: virtual clock, seeded RNG, event bus, sandbox FS,
                   RGBA screen device, config
  litebox-dalvik   The VM: bytecode pre-decoder, interpreter, object model
                   + GC, class registry, cooperative tasks, natives registry,
                   debugger state, snapshot serialization
  litebox-android  Framework shims + natives, view renderer, AXML manifest
                   parser, package manager
  litebox-agent    Control plane: service dispatcher, JSON-RPC HTTP + WS,
                   MCP (stdio + streamable-http), CLI + REPL, PNG encoder
```

Design notes:

- The pre-decoder resolves pool references once per method at class load;
  the hot loop dispatches on a flat instruction enum with inline caches
  patched on first execution (direct targets, field slots).
- Registers are swapped out of the frame per instruction so the heap,
  registry, and scheduler can mutate freely inside any opcode arm.
- GC is deferred: allocation pressure sets a flag, collection runs between
  instructions with the full root set (task frames, statics, interned
  strings, exception tables).
- Instance field slots are inherited-first, so a field's slot is stable across
  the whole subclass hierarchy.
- All state that an app can see lives in the Machine, which is why snapshot
  save/restore can capture and replay everything.

## Tests

`cargo test` runs 50 tests across 17 suites: JSON/ZIP/DEX round-trips (cross-
checked against Google's `dexdump` and Python's `zlib`), kernel primitives,
interpreter execution of toolchain-built bytecode (recursion, wide ops,
interfaces, exceptions, activity lifecycle, framebuffer pixels), and snapshot
round-trips into fresh machines.

Benchmark: `cargo run -p litebox-android --example bench-vm` — measured on
this machine: 18.1M instructions/s (recursion-heavy), 5.2M/s (array-heavy),
release build.

## Known limitations

- No native code execution (`.so`, JNI beyond shims).
- No layout/XML inflation; views must be constructed in code (or via a shim).
- String resource lookup (`Context.getString`) is not implemented.
- The event ring is bounded; the oldest events are dropped first.
- `Thread` support is cooperative: no preemption, no data races by design,
  busy loops still block their task's slice.
- Regexp-dependent `String.match`/`String.replaceAll` are not implemented
  (literal `replace` is).

## License

MIT. See `LICENSE` and `CHANGELOG.md`.
