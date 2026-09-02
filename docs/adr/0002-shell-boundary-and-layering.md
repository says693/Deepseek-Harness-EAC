# ADR 0002: AIO shell boundary and layering

## Status

Accepted for the `aio-v1` maintenance line.

## Context

AIO v1 is based on the historical `v4.5-lite` line and does not share the current 5.x `dsh-desktop` / `tauri-shell` source layout. It still needs an explicit native/web/runtime boundary so changes can be classified and verified consistently.

## Decision

AIO uses three layers:

1. **L1 — `tauri-app` Rust shell**
   - process, window, tray, watchdog and installer lifecycle;
   - Tauri ACL and per-command runtime-origin verification;
   - only a child-process ready line can establish the trusted web origin.
2. **L2 — `sidecar` Node service**
   - profile/preset synchronization, plugin guard and plugin update logic;
   - stdio JSON-RPC; no direct Electron dependency.
3. **L3 — bundled DeepSeek Harness runtime**
   - launched through the bundled Node executable;
   - web service bound to loopback and a dynamically selected port.

AIO data is isolated by default:

- installed data: Tauri app data for `com.deepseek.dsh.desktop.aio`;
- portable data: `.dsh-aio-data` next to the executable;
- legacy localStorage import: disabled unless `DSH_AIO_IMPORT_LEGACY=1`;
- explicit `DSH_HOME` / `DSH_DESKTOP_USERDATA`: treated as an intentional user override.

The current 5.x development Skill may validate either the modern layout or this AIO layout. Equivalent AIO smoke entrypoints live at the repository root and under `tauri-shell/` for validation compatibility; they delegate to the real `tauri-app` implementation.

## Consequences

- AIO changes must not be merged into the current 5.x `main` as a drop-in replacement.
- Release packaging requires a separately reviewed offline profile seed via `DSH_PROFILE_SEED_DIR`.
- Runtime/package changes require JavaScript tests, Rust tests, boot/GUI/update smoke, staging, portable layout verification and an installation transaction test.
- Client self-update remains absent in AIO; update smoke verifies the absence and checks that plugin auto-update defaults to disabled.
