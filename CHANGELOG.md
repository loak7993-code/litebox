# Changelog

All notable changes to LiteBox are documented here. Versions follow semver.

## [0.1.0] - 2026-10-11

First release: a headless Android app runtime for AI agents.

### Added
- DEX virtual machine: parses real APKs (classes.dex, multi-dex), pre-decodes
  Dalvik bytecode into resolved instruction streams, and executes it with
  inline caches for direct-invoke and field targets.
- Class registry with lazy structural resolution, inherited-first instance
  layouts, virtual/interface dispatch maps, and compacting mark-and-sweep GC
  driven by an allocation budget.
- Cooperative green-thread scheduler with deterministic virtual time: sleeps,
  joins, monitor wait/notify, and handler posts all advance on the VM clock.
- Android framework shims: java.lang (String, StringBuilder, boxed types,
  Math, System, throwables), java.util (ArrayList + iterator, HashMap,
  HashSet, Arrays, Objects, Random), threads, and android.*
  (Log, Build, Context, Activity lifecycle, View/TextView/Button/EditText/
  LinearLayout, Intent, Bundle, Handler/Looper).
- Software view renderer that draws the shim view tree into an RGBA
  framebuffer; screenshots export as PNG.
- APK package manager: install, list, uninstall; binary AndroidManifest
  (AXML) parser for package name, version, and launcher activity.
- Debugger: pause (manual, exception, unsupported-API), instruction stepping,
  method-level and instruction-level breakpoints, frame/register/object
  inspection, and runtime field patches.
- Snapshot save/restore of the entire VM state (heap, tasks, scheduler,
  clock, RNG, filesystem, event log) for crash replay.
- Control plane with one dispatcher behind three surfaces: JSON-RPC 2.0 over
  HTTP, MCP (stdio and streamable-http), and a CLI with an interactive
  debugger REPL. WebSocket logcat streaming.
- Zero third-party dependencies: JSON, ZIP/DEFLATE, CRC32, SHA-1, base64,
  and PNG encoding are implemented in-tree.
- Smoke-test APK built with the real Android toolchain (javac, d8, aapt2,
  apksigner) plus dex-level cross-verification against Google's dexdump.
