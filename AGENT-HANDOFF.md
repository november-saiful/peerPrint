# peerPrint — Agent Handoff

**Purpose:** This file is the single source of truth for continuing work on peerPrint from another PC.
Read it fully before changing code. It records what the app is, what the user asked for, what has
been verified, what is only probed, and the exact migration plan and commands.

Last updated: 2026-11-10 (notification flyouts/center, History tab, borderless title bar, update checks).

> **2026-11-09 update — UI decision reversed.** The Slint 1.18.1 migration was implemented and
> verified, then reverted at the user's request: the Slint Royalty-free 2.0 license requires a
> visible "Made with Slint" attribution and the user rejected it. egui/eframe 0.37 (MIT) is the
> shipped UI again. `src/ui/*.rs`, `Cargo.toml`, and `build.rs` are back on egui. Sections §1, §5,
> §6, §7 and §12 are **historical** — do not restart the migration. The egui shell handles
> tray/IPC/lifecycle work in `eframe::App::logic()` so it keeps working while the window is
> minimized or hidden (eframe does not call `ui()` then).
>
> **2026-11-10 update — visual redesign (UI only).** The whole window was rebuilt around
> `src\ui\theme.rs`: a system-aware light/dark palette, Segoe UI Variable loaded from
> `C:\Windows\Fonts` (no fonts shipped), a semibold family (`peerprint-bold`), a 208 px sidebar
> with Host/Client/Settings, card/list rows, status pills, animated toggles, and a modal pairing
> dialog. Every network/protocol/spooler/config behavior is unchanged; only `eframe::App::ui()`
> and the five view modules changed, plus one `ui::theme::install(&cc.egui_ctx)` call in
> `main.rs`. Verified on this PC with a scratch `PEERPRINT_DATA_DIR` at 1000×720 and at the
> 820×600 minimum: all three tabs, honest empty states, 11/11 tests. Not yet seen live: a
> populated host/clients/job list, the enabled-host card (needs a shareable USB printer), the
> pairing modal (needs a discovered host), and dark mode (follows the Windows theme). Scratch
> data dir, logs, and the `HKCU\...\Run\peerPrint` entry written by test launches were removed
> afterwards; the entry is recreated by the app on the next launch while `cfg.autostart` is true.

>
> **2026-11-10 cleanup.** Every scratch/probe artifact was deleted at the user's request: all of
> `.tmp\` (screenshots, PowerShell UI/tray automation scripts, scratch data dirs, `.bak` and
> `-orig` backups, build logs), the archived Slint UI source, the throwaway Slint probe project,
> the original project-brief markdown, and `target\` (11 GB of regenerable build output). Run
> `cargo build` to recreate `target\`. References to those paths below are historical.
>
> **2026-11-10 update — job cancel shipped (including remote).** Client queued/uploading jobs can
> be cancelled from the Job history table (`src\client.rs::cancel_client_job`, outbox files
> removed, `JobCancel` sent if a transfer is in progress). Host-side jobs can be cancelled while
> Receiving/Spooling/Printing (`Shared::request_cancel` + the 500 ms poll in
> `host.rs::track_spool_job` calls `spooler::cancel_job`). A client job already printing can now
> be cancelled too: the host keeps a cancel-safe socket read open during spool tracking
> (`net\framing.rs::MsgReader`), so `ClientMsg::JobCancel` stops the job and both sides record
> `Cancelled`. Retry/backoff and upload dedupe were re-reviewed (§9). Release build measured (§9).
>
> **2026-11-10 update — all-users install: Windows service + engine/UI split (implemented; engine and
> UI verified as two processes).** The user chose a pre-login, machine-wide install and asked the
> installer to offer **mode** (All users vs Just me) and **role** (Host / Client / Both).
> - `src\backend.rs` is new. `Backend` is the UI's facade: `Backend::local(shared)` wraps the
>   in-process engine exactly as before (single-user install), `Backend::remote(runtime)` polls a
>   machine-wide engine every 600 ms over `\\.\pipe\peerprint-engine`. The wire format is one
>   newline-delimited JSON `Request`/`Response` per connection; `Snapshot` carries config, `CoreState`,
>   paired clients, identity and firewall state. Commands (set_config, refresh, cancel, revoke,
>   pair, remove, test page, probe, firewall, notices) are replayed by the engine.
> - `src\service.rs` takes `start_services(shared, interactive, engine)`; the engine flag starts the
>   pipe server instead of the per-user control pipe, and `config::Role::hosts()/clients()` gate
>   host+advertiser vs browser+relay supervisor. The UI hides tabs and disables the tray hosting
>   toggle when the role forbids it.
> - Machine-wide state lives in `%ProgramData%\peerPrint` (`config::set_machine_wide(true)`), with
>   machine-scope DPAPI for secrets. The engine pipe DACL is
>   `D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;BU)` so standard users can talk to a LocalSystem engine.
> - `src\win\service.rs` (new) registers a boot-start `SERVICE_WIN32_OWN_PROCESS` named
>   `peerPrint` as LocalSystem with a `Spooler` dependency. `--install-service` also restarts an
>   existing service so upgrades take effect, locks the ProgramData ACL to SYSTEM + Administrators
>   via `icacls`, and `--uninstall-service` stops (10 s budget) and deletes it. Because it runs
>   elevated from session 0, `client::run_elevated_worker` now runs spooler installers in-process
>   when already elevated instead of ShellExecute `runas`.
> - Hidden flags: `--machine-wide`, `--service`, `--engine` (headless engine without the SCM, for
>   testing), `--set-role host|client|both`, `--install-service`, `--uninstall-service`.
> - The machine-wide UI is a thin tray client: startup shows "Windows service / Running" from the
>   pipe connection, diagnostics show "Engine: Background service" and the ProgramData path, and
>   "Quit tray app" exits only the UI. UI logs fall back to `%LOCALAPPDATA%\PeerPrint\logs` when
>   ProgramData is locked.
> - Verified on this PC with a normal (non-elevated) test engine: snapshot over the pipe
>   (`--diagnose --machine-wide`), UI connect, a remote Host-port write (39100→39120→39100), role
>   `host` (only Host + Settings tabs) and role `client` (only Client + Settings, "Ready / 0 relays
>   active"), UI quit while the engine stayed up, hidden `--autostart` UI raised by a second launch,
>   the disconnected banner with no engine, and `cargo test` 11/11. **Not yet run:** the actual
>   SCM registration/start (needs an elevated shell) and `iscc` packaging (Inno Setup 6 not installed).
> - `installer\peerprint.iss` was rewritten: `PrivilegesRequired=admin` +
>   `PrivilegesRequiredOverridesAllowed=dialog` gives the All users / Just me mode dialog;
>   `{autopf}`/`{autoprograms}` map paths per mode; a wizard page stores the role via
>   `--set-role`; all-users runs `--install-service` and launches the UI with `runasoriginaluser`
>   `--machine-wide`; uninstall removes the service then runs `--uninstall-all [--machine-wide]`.
>   It compiles on this PC as of 2026-11-10 (Inno Setup 6.7.3, `ISCC.exe` under
>   `%LOCALAPPDATA%\Programs\Inno Setup 6`) to `dist\peerPrint-0.1.0-setup.exe` once the variable
>   type is `TInputOptionWizardPage`. A silent `/ALLUSERS` install+uninstall cycle was verified:
>   install lands in `C:\Program Files\peerPrint`, re-points the service there (RUNNING), creates the
>   all-users Start Menu shortcut with `--machine-wide`, writes the HKLM uninstall key and applies
>   the default role `both`; uninstall removes the service, files, shortcut, registry key, HKCU Run
>   value and the `ProgramData\peerPrint` contents. The interactive wizard and Just me mode were not
>   exercised.
>
> **2026-11-10 fix — uninstall skipped queue removal.** `client::run_elevated_worker`'s in-process
> dispatch handled `--install-client` and `--uninstall-client` but not `--uninstall-all-worker`, so
> when the caller was already elevated (the uninstaller, or an admin shell) `--uninstall-all`
> returned exit 2 and skipped both queue removal and the data purge; the uninstaller still deleted
> the program files. Added the missing arm. Verified by removing a leftover client queue
> (`Canon G2010 series (peerPrint)` on port `PeerPrint_e8ce4b22dc27`) and by a full
> reinstall → uninstall cycle: both `[UninstallRun]` steps exit 0, the real `Canon G2010 series`
> (USB001) queue is untouched, and `cargo test` is 11/11.
>
> **2026-11-10 fix — no console window for the tray UI.** `src\main.rs` is now
> `#![windows_subsystem = "windows"]` and `main()` calls `attach_parent_console()`
> (`AttachConsole(ATTACH_PARENT_PROCESS)`) before clap, so terminal launches still print
> `--diagnose`, `--quit`, `--install-service` and `--help`. Verified on this PC: the release UI
> opens with no console/Windows Terminal window, Settings still shows "Windows service / Running",
> and `--diagnose --machine-wide` returns the service snapshot. Also removed a stale
> `HKCU\...\Run\peerPrint` value that pointed at `target\debug\peerprint.exe --autostart` and stole
> the single-instance slot at logon. Caveat: cmd.exe does not wait for GUI-subsystem programs and
> hands them no inherited stdio, so `peerprint --help > file` from a cmd prompt writes nothing;
> redirect via PowerShell (`Start-Process -RedirectStandardOutput`) or a pipe instead. The service
> registered here still points at `D:\peerPrint\target\release\peerprint.exe` — the installer is
> what moves it under Program Files. Test-engine service start via SCM was verified, but a real
> reboot (boot-start) has not been.
>
> **2026-11-10 update — notifications: 5-second bottom-left flyouts + notification center; new History tab.**
> Requested by the user: all notification flyouts last only 5 seconds and float at the bottom left;
> the same notifications stay reachable through a bell/button popup at the bottom left; print
> history moves to its own tab.
> - `service.rs`: `Notice` gained `id: u64` (serde default 0) so a UI can tell new notices from old
>   ones across engine restarts. `Shared::notice_seq` is an `AtomicU64` seeded from
>   `unix_ms * 1000`; each notice gets `fetch_add(1)` and the deque is truncated to **50** (was 20).
>   `Shared::clear_notices()` was added; `backend::Request::ClearNotices` + `Backend::clear_notices()`
>   make Clear work for both local and remote (pipe) backends.
> - `src\ui\notifications.rs` (new): 5.0 s flyouts (`FLYOUT_SECONDS`), 350 ms fade, 340 px wide,
>   max 4 stacked; an `Area` anchored `LEFT_BOTTOM` to the sidebar bell at `Order::Foreground`, so
>   they float over the content; each card has a × hitbox. A bell row at the bottom of the sidebar
>   shows an unread badge (count of `id > seen_notice`, capped "9+"). Clicking it opens a 360 px
>   center above the bell with every retained notice, relative age, Clear, and an honest empty
>   state; outside click/Escape closes it. Flyouts are suppressed while the center is open.
> - Read/delivery cursors are separate: `seen_notice` drives the badge and only advances when the
>   center is opened (or a notice arrives while it is open); `flyout_notice` is the delivery cursor
>   so each notice flies out exactly once and the badge still counts it afterwards. The first
>   connected snapshot only sets the baseline — restarts do not replay old notices as flyouts.
> - `src\ui\history_view.rs` (new) + `Tab::History` (between Client and Settings): chips All jobs /
>   Received (host) / Sent (client), role-gated with a stale-filter reset, "{n} jobs kept" on the
>   right, and the existing `jobs_panel` rows. `jobs_panel` adds a Received/Sent direction pill only
>   in the unfiltered All jobs view. The embedded "Job history" sections were removed from
>   `host_view.rs` and `client_view.rs`.
> - Notices remain **in memory only**: 50 max, wiped when the service/engine restarts. There is no
>   persistence and nothing is written to disk for flyouts.
> - Follow-up polish (2026-11-10): the active/hover sidebar nav row and the notification row are
>   now full pills (radius 19) instead of 10 px rectangles, and the bell is the native Segoe Fluent
>   Icons `EA8F` glyph instead of the hand-painted bell. `theme.rs` loads `SegoeIcons.ttf` (or
>   `segmdl2.ttf`) as the `peerprint-icons` family — no files shipped — and
>   `theme::icon_font()` returns `None` if neither exists, in which case the old painter bell is the
>   fallback. Verified visually on the debug build (nav pill, bell glyph, open-center pill state).
> - Verified with real clicks + PrintWindow captures on the debug build connected to the installed
>   service: flyout appears and disappears after 5 s, badge persists until the center is opened,
>   center lists the retained notices, Clear empties it, History shows the three real jobs from the
>   earlier two-PC test with Sent pills, and all three chips filter correctly (Received → honest
>   "No jobs yet" because every retained job was sent). `cargo test` 11/11. Screenshot-automation
>   gotchas found here are in §8.

---

## 0. Start here on the new PC

Prerequisites:

- Windows 10/11 x64.
- Rust stable (MSVC toolchain) with Visual Studio Build Tools (Desktop C++ + Windows SDK).
  Old PC had `cargo 1.99.1`; `rust-toolchain.toml` pins channel `stable`, target `x86_64-pc-windows-msvc`.
- Node is not required for the Rust app.

Build and test:

```powershell
cd <project root>            # e.g. E:\peerPrint
cargo build
cargo test                   # last known: 11/11 passing
cargo run -- --diagnose      # prints JSON state, does not start the UI
```

Run the app (current egui build):

```powershell
cargo run
cargo run -- --autostart     # starts hidden in the tray (login path)
cargo run -- --background    # no window at all, console waits (headless path)
cargo run -- --quit          # asks a running instance to quit

# machine-wide engine/UI split (test paths; --service is for the SCM)
peerprint.exe --engine                            # headless engine, ProgramData state, Ctrl+C stops
peerprint.exe --machine-wide                      # thin tray UI that talks to the engine pipe
peerprint.exe --diagnose --machine-wide           # prints the engine Snapshot JSON from the pipe
peerprint.exe --set-role host|client|both [--machine-wide]
peerprint.exe --install-service                   # elevated; registers + starts the boot service
peerprint.exe --uninstall-service                 # elevated; stops + deletes it
```

Build artifacts: `target\` is regenerable (was 4.5–11 GB) and was deleted in the 2026-11-10
cleanup. Do **not** copy `target\` to the flash drive. `Cargo.lock` is present and must be copied.

Copying to a flash drive (run from a shell on the old PC):

```powershell
robocopy D:\peerPrint E:\peerPrint /E /XD "D:\peerPrint\target"
```

The tree is now only source + docs + assets + installer, so this copies everything the new PC
needs. The first `cargo build` there will download crates and recompile (slow, several minutes).

Data directory (on whatever PC runs it): `%LOCALAPPDATA%\PeerPrint` for a single-user install;
`%ProgramData%\peerPrint` for an all-users install (machine-wide flag/service), overridable with the
`PEERPRINT_DATA_DIR` environment variable. A fresh PC gets a fresh identity/certificate and has no
paired clients or saved queues; pairing and two-PC testing must be redone from scratch.

---

## 1. Historical: UI framework decision (settled — egui)

Background for the record only; the decision is final.

The user asked for a WinUI3-style UI and Slint 1.18.1 was tried first (Iced was rejected because its
release had no window show/hide API, which breaks close-to-tray). Slint was implemented, verified,
then **reverted on 2026-11-09** because its Royalty-free 2.0 license requires visible "Made with
Slint" attribution, which the user rejected. The shipped UI is egui/eframe 0.37 (MIT). Do not
re-start this migration.

Licensing constraint (**historical** — resolved by reverting to egui on 2026-11-09): Slint's
Royalty-free 2.0 required an **AboutSlint** widget in an About screen or a badge on the public
download page. The user rejected the badge, so Slint was removed; egui/eframe 0.37 is MIT and needs
no UI attribution. Keep the MIT license notice in the installer/about text as before.

Standing user constraints:

- Never ship invented inventory/data. Wire real APIs or show an honest empty/handoff state.
- Do not disturb the user's real printer queue: **"Canon G2010 series"** (and the peerPrint queue
  `Canon G2010 series (peerPrint)` on the old PC). Keep it intact.
- `--uninstall-all` is destructive: do not run it without explicit approval.
- The two-PC discovery/pairing/print test needs the user's second machine; do not fake it.

Working style the user prefers: plain-English status, concise updates, honest verification claims.
A "what next?" question means give a short status + next step, not a design essay.

---

## 2. Current state (be precise)

| Item | State |
|---|---|
| Real app UI | **egui/eframe 0.37 (restored 2026-11-09)**; Slint dependencies removed. |
| Slint port | **Reverted.** Source was archived in `.tmp\slint-ui\` and deleted in the 2026-11-10 cleanup (do not re-add). |
| Slint probe | Archived reference only; not part of the build. |
| Tray menu (egui) | Verified again 2026-11-09 with real clicks: Open / Share / Quit, restore from minimized, actions while minimized, `--quit` while hidden. |
| Tests | **11/11** `cargo test` passing on the final tree (verified 2026-11-10; added retry-backoff and two cancel-safe-reader tests). |
| Job cancel | Implemented 2026-11-10, including remote cancel while printing. Queued/uploading client cancel verified with a scratch outbox; the cancel UI for printing jobs verified with a scratch fixture (banner shown, state unchanged without a host). Host-side and remote-cancel network paths are code-reviewed + unit-tested only (no shared printer or second PC on this machine). |
| Notifications / History tab | 5 s bottom-left flyouts, sidebar bell with unread badge, popup center with Clear, and a dedicated History tab with All/Received/Sent filters. Verified 2026-11-10 by real clicks on the debug build against the installed service. Notices are memory-only (50 max, reset with the service). |
| Release build | `cargo build --release` clean (11 pre-existing dead-code warnings). Binary 12,187,648 bytes. RAM on a fresh window: **97.6 MB working set / 53.0 MB private / 22 threads** (after 20 s idle: 95.6 / 51.2 / 22). See §9 verification log. |
| Installer | Compiles with Inno Setup 6.7.3 to `dist\peerPrint-0.1.0-setup.exe` (5.7 MB). All users install **and** uninstall verified 2026-11-10 (silent); Just me mode and the interactive wizard pages are untested. |
| Two-PC pairing/print | **Blocked** — needs the user's second PC; must not be simulated. |
| UI migration tasks #11–#14 | **Obsolete** — migration abandoned. |

The app source is a single Rust binary (`peerprint`) with modules listed in §4.

---

## 3. Product/architecture summary

peerPrint shares a USB printer from a host PC to client PCs on the same Wi-Fi, without SMB/workgroups:

```
Any Windows app
  → client Windows spooler
  → local TCP/IP port 127.0.0.1:391xx (SNMP off, BIDI off)
  → peerPrint client relay (job buffered to disk)
  → TLS 1.3 + per-client token over Wi-Fi
  → peerPrint host
  → host Windows spooler (RAW)
  → USB printer
```

Runtime shape:

- Host: tokio services + mDNS advertisement + TLS server + local spooler integration.
- Client: discovers hosts, pairs with SPAKE2/PIN, installs a Windows print queue at an elevated
  helper step, runs a localhost relay per saved printer that buffers jobs on disk when the host is
  unreachable.
- UI: one desktop process; tray icon; window close hides to tray; `--autostart` starts hidden;
  `--background` runs with no window at all.

Single instance: named mutex + named pipe. Launching again sends "show"; `--quit` sends "quit".
The UI polls `crate::ipc::take_activation()` and `crate::ipc::take_quit()` from
`eframe::App::logic()`, which runs even while the window is minimized or hidden.

Process lifecycle in `src/main.rs` (around lines 177–224):

1. `service::start_services(shared, interactive)`
2. `client::resume_all_queues(&shared)`
3. run UI (`eframe::run_native`)
4. after UI exits: `service::shutdown(&shared)`, `client::pause_all_queues(&shared)`,
   `runtime.shutdown_timeout(2s)`.

Keep this order. Tray **Quit** must exit the event loop, not call `std::process::exit`, so
shutdown/pause still runs.

---

## 4. Source map and service API cheat sheet

| Path | LOC | Role |
|---|---|---|
| `src/main.rs` | 342 | CLI, lifecycle, eframe startup |
| `src/service.rs` | 600 | `Shared`, `CoreState`, jobs, notices, relays, printer refresh, cancel flags |
| `src/config.rs` | 420 | `AppConfig`, host/client config, DPAPI PIN, paired clients, data dir |
| `src/client.rs` | 1720 | Discovery/pairing/queue install/relay/jobs, outbox + cancel |
| `src/host.rs` | 944 | Host service, transfer, spool tracking, cancel polling (line ~63: `let wanted = cfg.host.enabled && !cfg.host.shared.is_empty();`) |
| `src/discovery.rs` | | mDNS + `probe_manual(shared, addr)` |
| `src/ipc.rs` | | single-instance pipe, `take_activation()`, `take_quit()` |
| `src/ui/mod.rs` | 589 | egui app shell: sidebar + central content, alerts, tray sync, close-to-tray, shell logic |
| `src/ui/notifications.rs` | 413 | 5 s bottom-left flyouts, sidebar bell + unread badge, notification center popup |
| `src/ui/history_view.rs` | 84 | History tab with All jobs / Received / Sent filters |
| `src/ui/theme.rs` | 506 | Design system: palettes, fonts, styles, cards, pills, buttons, toggle, inputs, modal frame |
| `src/ui/host_view.rs` | 435 | Host page |
| `src/ui/client_view.rs` | 289 | Client page |
| `src/ui/settings.rs` | 258 | Settings + diagnostics |
| `src/ui/pairing_dialog.rs` | 142 | Add-printer modal (`egui::Modal`) |
| `src/ui/jobs_panel.rs` | 124 | Job rows + Cancel button + Received/Sent direction pill |
| `src/ui/tray.rs` | 117 | `tray-icon` crate tray |
| `src/update.rs` | | GitHub Releases update check, version comparison, release-page opener |
| `src/win/*` | | spooler, elevation, firewall, autostart, DPAPI, single instance |
| `assets/peerprint-256.png`, `.ico`, `.manifest`, `.rc` | | icon/resource embedding via `build.rs` |

### `Shared` API used by the UI

```rust
shared.config() -> AppConfig                 // clone
shared.core() -> CoreState                   // clone
shared.set_config(cfg) -> Result<()>         // persist + replace
shared.notice(NoticeLevel, message)          // push notice (keeps 50, assigns monotonic id)
shared.clear_notices()                       // wipe the in-memory notice history
shared.refresh_local_printers()              // blocking Win32 spooler enumeration
shared.paired_clients() -> Vec<PairedClient>
shared.clients.lock() -> PairedClients       // .revoke(id)
shared.identity.host_id: Uuid
shared.identity.fingerprint: [u8; 32]        // hex for UI
shared.runtime: tokio::runtime::Handle
```

`CoreState` fields: `host_running: bool`, `host_error: Option<String>`,
`local_printers: Vec<LocalPrinterView>`, `printer_status: HashMap<Uuid, PrinterStatusInfo>`,
`discovered: Vec<DiscoveredHost>`, `jobs: Vec<JobRecord>`, `relays: Vec<RelayView>`,
`notices: VecDeque<Notice>`.

View types:

```rust
LocalPrinterView { name, port_name, driver_name, driver_env, driver_version, hardware_id,
                   status: u32, status_text, online, jobs: u32, shareable }
DiscoveredHost { host_id: Uuid, instance, addr, display_name, pin_required, tls_fp: String,
                 printer_count, printers: Vec<PrinterMeta>, last_seen }
PrinterMeta { id: Uuid, name, model, driver_name, driver_env, driver_version, online, status, jobs }
RelayView { saved_id: Uuid, queue_name, port: u16, listening, last_error, last_job }
JobRecord { id, direction: Host|Client, printer_name, peer, doc_name, bytes, state, pages,
            error, created, finished }
Notice { id: u64, level: Info|Warn|Error, message, at }
```

Config types relevant to the UI:

```rust
AppConfig { autostart: bool, host: HostConfig, client: ClientConfig }
HostConfig { enabled, display_name, listen_port: u16, require_pin: bool, pin_dpapi: Option<String>,
             shared: Vec<SharedPrinter>, firewall_configured: bool }
SharedPrinter { id: Uuid, spooler_name, display_name, model, driver_name, driver_env,
                driver_version, port_name, enabled }
ClientConfig { port_base: u16, outbox_limit_mb: u64, saved: Vec<SavedPrinter> }
SavedPrinter { id, client_id, host_id: String, host_addr, host_display, printer_id, printer_name,
               printer_model, driver_name, driver_env, relay_port, port_name, queue_name,
               tls_fp, token_dpapi, installed }
PairedClient { id, name, token_hash, created_at, last_seen, revoked }
```

Key behavior detail: the tray "sharing" state is derived from `cfg.host.enabled`, **not**
`core.host_running`. The host only runs when enabled **and** at least one printer is shared
(`host.rs:63`), so the old `core.host_running` check never showed the checked menu item when no
printer was selected. The egui code derives it in `src/ui/mod.rs` (`ensure_tray` and the tray
action handler); keep that derivation if the tray is touched.

### UI theme cheat sheet (`src/ui/theme.rs`)

`theme::install(ctx)` is called once in `main.rs` inside `run_native`. It loads
`SegUIVar.ttf`/`segoeui.ttf` as the proportional family and `seguisb.ttf` as
`FontFamily::Name("peerprint-bold")`, then applies matching light and dark styles via
`ctx.style_mut_of(Theme::{Light,Dark}, …)` so the app follows the Windows theme. It also loads
`SegoeIcons.ttf` (fallback `segmdl2.ttf`) as `FontFamily::Name("peerprint-icons")`; use
`theme::icon_font()` and keep a painter fallback because the font is optional. No font files are
shipped, and there is no runtime theme switcher — do not add one without the user asking.

The accent color comes from the logo (`assets/peerprint.svg`): leaf green `#92C13E` for fills
(primary buttons, toggle track, selection, active dot) in both themes. `accent_text` is a darker
leaf (`#4E7415`) used for text/caret/hyperlinks on light surfaces because `#92C13E` on white is
only about 2:1 contrast; `on_accent` is a near-black olive for text drawn on accent fills.

Helpers used by all views: `palette(ui)`, `bold()`, `page_header`, `section_title`, `card`,
`list_card`/`list_row` (rows + `theme::divider` between them), `empty_card`, `notice`,
`inline_warning`, `pill`, `primary_button`/`secondary_button`/`danger_button`/`tiny_button`,
`toggle`, `text_input`/`password_input`, `label`/`muted`, `dialog_frame`, and
`state_color(&palette, job_state)`. `egui::Panel::left` is the sidebar (208 px) and the content
is a `CentralPanel` + vertical `ScrollArea`. Empty states are deliberately honest — never put
invented printers, hosts, or jobs into the UI for screenshots or demos.

### Borderless window with a custom title bar

The viewport is built with `with_decorations(false)` but keeps `.with_title("peerPrint")` and
`.with_icon(...)`. The real window text and icons are what Explorer reads for the taskbar button,
hover tooltip and thumbnail (`GetWindowText` / `WM_GETICON`), so they must never be blanked — doing
that earlier (to hide the native caption brand) broke the taskbar preview, which showed no name and
no icon. With the caption removed there is nothing on screen to duplicate the sidebar's leaf mark
and wordmark anyway.

`App::title_bar` (in `src/ui/mod.rs`) draws a 38 px `egui::Panel::top("app_titlebar")` filled with
`palette.sidebar`, so it is one surface with the sidebar. Left of the caption buttons is a
`Sense::click_and_drag` rect that sends `ViewportCommand::StartDrag` on
`drag_started_by(Primary)` and toggles `ViewportCommand::Maximized` on `double_clicked`. The three
buttons are custom `window_button` hits: minimize (`\u{E921}`, `Minimized(true)`), maximize/restore
(`\u{E922}`/`\u{E923}`, `Maximized(!maximized)`), close (`\u{E8BB}`, `Close`, which hides to tray
rather than exiting). Hover fills with `palette.border`; close hovers red with a white glyph. The
glyphs use `theme::icon_font()` (Segoe Fluent Icons) and are skipped if it is unavailable.

Undecorated winit windows lose native edge resizing, so `resize_edges(ctx, max_rect)` runs each
frame: a 6 px band on each edge (corners take priority) sets the resize cursor and sends
`ViewportCommand::BeginResize(dir)` on primary press. It is a no-op while maximized. Maximized
windows keep the 8 px Windows inflation, so the caption buttons sit at y≈19 still on screen.

`crate::win::titlebar::apply(frame, &palette)` now only sets `DWMWA_USE_IMMERSIVE_DARK_MODE` and
`DWMWA_BORDER_COLOR`, keeping DWM's drop shadow and rounded corners. It is called when the resolved
theme changes (cached in `App::window_theme`). Requires `Win32_Graphics_Dwm` plus
`raw-window-handle`; on systems without the attribute the call fails silently.

Verified on Windows 11 (debug build, light theme): taskbar title and all icon slots non-zero
(`icon_small`/`icon_small2`/`icon_big`), title-bar drag moves the window, double-click maximizes and
restores, edge drag resizes (846×720 checked), minimize/close behave (close leaves the process
running in the tray), DWM shadow and rounded corners intact. Synthetic taskbar *hover previews*
cannot be triggered on this machine (Explorer's own preview does not appear either), so the
thumbnail itself was not screenshotted — the shell inputs it reads are verified instead.

### Update checks (`src/update.rs`, `src/ui/mod.rs`, `src/ui/settings.rs`)

Notify-only by design: the app never downloads or executes an installer. `update::check()` requests
`https://github.com/november-saiful/peerPrint/releases/latest` with `redirects(0)` and reads the
`Location` header. GitHub's web redirect resolves to the newest non-draft, non-prerelease tag, so
the result has the right semantics without touching the REST API — important because the API allows
only 60 anonymous requests per hour per IP, and a shared ISP address exhausted it (403) on
2026-11-10. The target must be `https://github.com/november-saiful/peerPrint/releases/tag/<tag>`;
the tag is parsed as semver after an optional `v`. A 404 (no releases yet) means "up to date", not an
error. Release links must stay on this repository; opening uses `ShellExecuteW("open")`.

`App::maybe_check_updates` runs from `logic()` at most once per `update::CHECK_INTERVAL_SECS`
(24 h), only when `cfg.updates.auto_check` is on; failing silently except for the Settings status
text. `App::start_update_check` spawns `spawn_blocking(update::check)` on the backend runtime and
delivers `ActionDone::UpdateChecked` (which is deliberately not routed through `spawn_action`,
whose task result is `Result<(), String>`). On success it saves `updates.last_check`, clears
`updates.last_error`, caches the found release in `updates.available` (version + link) and posts one
Info notice per new version (`updates.last_notified_version` prevents repeats). A failure stores
the message in `updates.last_error`, so a restart inside the 24 h window keeps showing the error
instead of claiming "up to date"; both caches are re-validated at startup by `update::cached`. The
settings card shows available / checking / up to date or error, plus "Check now" and "View release";
its text column reserves 240 px for those buttons so long error text wraps instead of running
underneath them.

Tests use `PEERPRINT_UPDATE_API` to point the checker at a local fixture server that answers with a
`Location` header (or 404): `update::tests` cover tag normalization, newer/equal/older, foreign
repositories and non-https links, malformed tags, cached-release revalidation, and socket fixtures
that assert the 302 `Location` is read without being followed and that 404 is not an error. Release
publishing is `.github/workflows/release.yml` (tag `v*`, tag must equal `Cargo.toml`; builds the
binary, `iscc`, SHA-256 sums, GitHub Release); the installer accepts `/DMyAppVersion=<version>` so
CI can override the version define.


---

## 5. Historical: UI behavior inventory from the Slint plan (egui remains the UI)

### Shell / global

- Header/rail shows: Host status dot + label (`Hosting` / `Host off` / `Host error`), version.
- Tabs: Host, Client, Settings.
- Notices: up to 4 most recent `core.notices` + `core.host_error` banner. Levels: Info/Warn/Error.
- Close (title-bar X) hides to tray and posts an Info notice: "peerPrint keeps running in the
  notification area. Use Quit there to stop it."
- `--autostart` starts hidden; first activation/show must bring it to front.
- A second launch notification and `--quit` must be polled and honored.
- Window: 1000x720 today, min 820x600. WinUI3 look requested: custom 40px title bar, caption
  buttons, nav rail, cards, toggles, 8px radii.

### Host page

- Sharing toggle (`cfg.host.enabled`), immediate apply on toggle (old code saves on change).
- "Shown to others" text field (`cfg.host.display_name`).
- Port field (`cfg.host.listen_port`, range 1024–65530).
- "Require a PIN to pair" toggle; PIN draft field (4–12 ASCII digits); "Set PIN" / "Clear PIN";
  "PIN is set"/"no PIN set yet" state from `cfg.host.pin_dpapi`.
- Warnings:
  - Enabled + host not running + no shared printers → "Enable at least one printer below to start
    hosting."
  - Enabled + `core.host_error` → show error.
  - Enabled + `!cfg.host.firewall_configured` → firewall banner + "Set up firewall" button which
    runs the elevated helper `elevation::run_elevated("--firewall-setup")` via
    `tokio::task::spawn_blocking`; on success set `firewall_configured = true` and post a notice.
- Printers on this PC: list from `core.local_printers`; per row: share toggle (adds/removes a
  `SharedPrinter`; generates `Uuid::new_v4()` and copies driver/port metadata), name, status dot
  (green online / amber offline) + `status_text (N jobs)`, driver, editable "Name shown to
  clients" (only when shared), Refresh button when empty. Non-shareable names are listed at the
  bottom. Saving config also forces `host.enabled = false` if there are no printers and no shared
  entries.
- Paired PCs: `shared.paired_clients()`; columns PC / paired / last seen (use `format_age`) /
  Revoke. Revoke calls `clients.lock().revoke(id)` + `config::save_paired_clients`, notices on
  success/error; show "revoked" for revoked rows.
- Job history (Host direction) table: When, Printer, From/To, Document (truncate to 32 chars + …),
  Size (`format_bytes`), Pages (`-` if 0), State (colored by `status_color`), error on hover.

### Client page

- Found on this network: per `core.discovered` host card with display name, address, PIN chip;
  per printer row: online dot, name, model, status, and right-aligned Add button (disabled while a
  pairing is busy) or "installed" when a matching `cfg.client.saved` entry exists
  (`host_id == host.host_id.to_string() && printer_id == printer.id`).
- Add opens the pairing dialog seeded with host/printer id/name.
- Add by address: text field + "Find host"; parse `host:port` or bare host with default `:39100`;
  calls `discovery::probe_manual(&shared, addr)`; spinner while busy; warn notice on invalid input.
- Installed printers: per `cfg.client.saved`: queue name, model, `host_display · host_addr`,
  matching `core.relays` entry status:
  - listening → green "Relay listening on 127.0.0.1:{port}"
  - not listening with error → red error
  - no relay yet → gray "Relay starting…"
  - also show `relay.last_job` when present, and the note "Queued jobs stay on this PC until
    peerPrint can reach the host." when a relay exists, is not listening, and has an error.
  - buttons: "Test page" → `client::test_page(queue)` (busy per saved id, notice on success/error),
    "Remove" → `client::remove_printer(shared, id)` (busy per id).
- Job history (Client direction), same table.

### Pairing dialog (modal)

- Shows printer name, host display name, host SHA-256 fingerprint + Copy button, warning to compare
  out of band.
- If `host.pin_required`: PIN field; otherwise "This host does not require a PIN."
- Error banner; busy state with "Windows will ask for administrator permission…".
- "Install printer" enabled when not busy and (no PIN required or PIN non-empty); runs
  `client::add_printer(shared, &host, printer_id, &pin)` (elevated helper). "Cancel" closes.
  Closing on success only (result `Installed(printer_id, Ok)` clears the dialog).

### Settings

- Startup: autostart toggle (calls `win::autostart::set_enabled`, saves `cfg.autostart`, notices);
  note that it runs in background; "Quit peerPrint" button.
- Network: host port, first relay port (`client.port_base`), outbox limit MB
  (`client.outbox_limit_mb`, 10–4096); firewall rules present? + "Add firewall rules" (same
  elevated flow as Host, `firewall::has_rules()`).
- Diagnostics: version, elevated?, protected print mode warning, host ID, certificate fingerprint
  (hex monospace), data folder path; "Copy diagnostics" (pretty JSON built in
  `settings.rs:149-167`) and "Open data folder" (explorer.exe).
- **New required:** About card with version, description, and the Slint `AboutSlint` widget for
  license attribution.

### Tray

- Native notification-area icon from `assets/peerprint-256.png`.
- Left click opens the window.
- Menu: "Open peerPrint" / "Share my printers" ↔ "Stop sharing printers" / "Quit peerPrint".
  The middle item toggles `cfg.host.enabled` and saves; its **label is reactive** because Slint's
  checkable tray items currently do not render a checkmark on Windows (Slint issue #14419). Do not
  use `checkable`; use the action label instead.
- Tooltip: `peerPrint — sharing · N relay(s) active` when sharing, `peerPrint — sharing on (no
  printers selected)` when enabled but idle, `peerPrint — N relay(s) active` when not sharing.
- Quit must trigger the service shutdown path (see §3).

---

## 6. Historical: Slint probe notes (probe and scripts deleted 2026-11-10)

The probe project (`.tmp\slint-probe`), the archived Slint UI (`.tmp\slint-ui`) and every `.tmp\*.ps1`
automation script were deleted in the 2026-11-10 cleanup. The commands, file names and script names
below are historical and will not run; the findings are kept because they explain the egui/Win32
shell code that shipped.

Probe dependencies (`Cargo.toml`):

```toml
slint = { version = "1.18.1", features = ["raw-window-handle-06"] }
raw-window-handle = "0.6"
windows = { version = "0.62", features = [
  "Win32_Foundation",
  "Win32_Graphics_Dwm",
  "Win32_UI_WindowsAndMessaging",
  "Win32_UI_Input_KeyboardAndMouse",
  "Win32_UI_Shell",
] }
winreg = "0.56"

[build-dependencies]
slint-build = "1.18.1"
```

`build.rs`:

```rust
fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).unwrap();
}
```

### Verified working

- Fluent-rendered WinUI3-ish look: 220px nav rail, page headers, white cards, custom
  40x20 animated toggle, LineEdits, primary/secondary buttons, custom caption buttons.
- Title font fix: `Segoe UI Variable Display` falls back to a serif; use **`Segoe UI Variable`**
  for headings and `Segoe UI Variable Text` for body. (Verified by screenshot.)
- Caption hit testing: a full-width drag `TouchArea` must be declared **before** the
  `HorizontalLayout` with the caption buttons; otherwise it covers them and clicks are swallowed.
  After reorder, close hover shows red and close click hides the window (log `hidden`).
- Minimize button: real click fires the Rust callback, `ShowWindow(hwnd, SW_MINIMIZE)` returns
  non-zero, window minimizes. Restore re-renders correctly.
- Maximize button: fills the work area, **taskbar remains visible**. Maximize/restore toggle via
  `IsZoomed`.
- Taskbar presence: `WS_EX_APPWINDOW` is set (style_ex `0x40110`), taskbar button appears even
  though the window is `no-frame`.
- Hidden start: `--hidden` starts with no visible window, process stays alive in
  `run_event_loop_until_quit()`.
- Native tray on hidden start: icon is registered (appears in the overflow flyout with accessible
  name `peerPrint`).
- Tray menu: right-click menu is owned by the probe process; items are `Open peerPrint` /
  `Share my printers` / `Quit peerPrint`; clicking "Share my printers" logs `tray hosting=true`,
  the menu retitles to "Stop sharing printers", and the UI toggle updates.
- Tray "Open" after close/hide: real menu click (`Open peerPrint`) shows the previously hidden
  window; window renders correctly.
- Close-to-tray: X hides the window and logs `hidden`; the event loop keeps running.
- DWM: rounded corners (`DWMWA_WINDOW_CORNER_PREFERENCE = 2`) and immersive dark mode from
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme` are applied.

### Not verified at the time (historical; probe deleted)

1. **Edge resizing.** `WS_THICKFRAME` added at runtime did not enable resize. The probe now
   contains a `SetWindowSubclass` hit-test proc (`install_chrome` + `hit_test` in
   `.tmp\slint-probe\src\main.rs`) returning `HTLEFT/HTRIGHT/HTTOP/HTBOTTOM` + corners within 8px
   (skipped when `IsZoomed`). It was just compiled, not run. Test by launching the probe, forcing
   it to the foreground (`.tmp\bring3.ps1`), then dragging the right edge:
   ```powershell
   powershell -File .tmp\bring3.ps1 -Title "peerPrint probe" -Out "probe-resize-before.png"
   powershell -File .tmp\drag.ps1 -X1 <right-edge-1> -Y1 400 -X2 <right-edge+120> -Y2 400
   ```
   Confirm the window width changes and the content reflows. Also check the corners still look
   rounded and nothing draws a native frame. If it fails, move hit-testing into the real app's
   bridge anyway and debug there (same code).
2. **Minimum size.** Not implemented. Add `WM_GETMINMAXINFO` in the same subclass (scale
   `820x600` logical with `GetDpiForWindow`) or a Slint min-size property if supported.
3. **Tray tooltip on hover.** A 1.6s hover test showed nothing; Slint's tray tooltip may not be
   wired on Windows. Verify; if broken, it is cosmetic but should be noted/worked around.
4. **Double-click title bar to maximize.** Add a `double-clicked` handler on the drag `TouchArea`
   (Slint `TouchArea` supports it) or handle `WM_NCLBUTTONDBLCLK` in the subclass.
5. **Live theme switch.** Probe reads the registry once at startup. Decide whether to re-check on
   show or on a timer.

### Probe pitfalls learned (avoid repeating)

- `SystemTrayIcon` must be the **root** of its own exported component
  (`export component AppTray inherits SystemTrayIcon`). It cannot be a child of `AppWindow`.
- Tray menu titles can bind to properties (`hosting ? "Stop…" : "Share…"`) and update reactively.
- In Slint, `in` properties can't be bound from outside; use `in-out` where the parent binds into
  a child (e.g., window `dark <=> Theme.dark`), and make custom toggle emit-only to avoid breaking
  parent bindings.
- `Theme` must be `export global Theme` to be accessed from Rust (`ui.global::<Theme>()`).
- Slint 1.18 deprecates `in` → `in-out` on bindings; don't set both width and min-width on the same
  element (caused a compile error).
- `window.window_handle()` returns `slint::WindowHandle` directly; call
  `HasWindowHandle::window_handle(&handle)` to get `RawWindowHandle`.
- `ReleaseCapture` lives in `Win32::UI::Input::KeyboardAndMouse`; `SendMessageW` takes
  `Option<WPARAM>` / `Option<LPARAM>`.
- Scripting clicks: **SetForegroundWindow is unreliable.** Several earlier "blank window" and
  "click did nothing" failures were actually the probe being behind Explorer/Qoder, so
  `CopyFromScreen` captured the occluding window and clicks landed elsewhere. Use
  `.tmp\bring3.ps1` (AttachThreadInput + BringWindowToTop + topmost pulse) and check its
  `foreground=True` output before trusting any click/capture. `.tmp\bring2.ps1` is less reliable.
- `Get-Process <name>.MainWindowHandle` can point at the wrong HWND (e.g., a lingering `#32768`
  menu). Find windows by title via `EnumWindows`.
- Output written from inside a native `EnumWindows` delegate is lost; buffer lines in an
  `ArrayList` and print after the enumeration (see `.tmp\enum-visible.ps1`).
- Tray menu automation: right-click the icon, find the visible `#32768` window owned by the target
  PID, send `MN_GETHMENU (0x01E1)`, then use `GetMenuItemRect` / `GetMenuStringW` before clicking.
  See `.tmp\tray-menu-item.ps1`; real rects matter.
- The probe window title is `"peerPrint probe"`; scripts key off that. The real app's title is
  `"peerPrint"`.

Reusable scripts in `.tmp\` (all deleted 2026-11-10): `bring3.ps1` (foreground + capture),
`probe-minrestore.ps1`, `enum-visible.ps1`, `probe-tray-find.ps1`, `tray-menu-item.ps1`,
`click-at.ps1`, `drag.ps1`, `shot.ps1`, `crop.ps1`. Re-create them from the descriptions above if
UI automation is needed again. `bring3.ps1` saved `-Out` relative to the working directory.

---

## 7. Historical: Slint migration (abandoned 2026-11-09)

> Kept for reference only. The migration was completed and verified, then reverted. Do not execute
> this plan. The app builds on egui/eframe 0.37; the archived Slint UI source was deleted in the
> 2026-11-10 cleanup.

### 7.1 Dependencies and build

Replace in `Cargo.toml`:

- Remove: `eframe`, `egui_extras`, `tray-icon`.
- Add: `slint = { version = "1.18.1", features = ["raw-window-handle-06"] }`,
  `raw-window-handle = "0.6"`.
- Extend `windows` features with: `Win32_Graphics_Dwm`, `Win32_UI_Shell`,
  `Win32_UI_Input_KeyboardAndMouse`. (`Win32_UI_WindowsAndMessaging` is already there.)
- Add build-dependency `slint-build = "1.18.1"`.
- Remove the now-unused `egui` imports from `src/main.rs` and delete `src/ui/*.rs` egui modules
  after porting.

`build.rs` must keep the existing resource embedding **and** compile the Slint file:

```rust
fn main() {
    println!("cargo:rerun-if-changed=assets/peerprint.ico");
    println!("cargo:rerun-if-changed=assets/peerprint.manifest");
    println!("cargo:rerun-if-changed=assets/peerprint.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let _ = embed_resource::compile("assets/peerprint.rc", embed_resource::NONE);
    }
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).unwrap();
}
```

Keep `slint::include_modules!();` in `main.rs` (or the new UI module root).

### 7.2 File layout

- `ui/app.slint` — new; copy the probe's design foundation, then build out pages.
- `ui/peerprint.png` — copy of `assets/peerprint-256.png` for `@image-url` (Slint resolves image
  paths relative to the `.slint` file; alternatively reference `../assets/peerprint-256.png`).
- `src/ui/mod.rs` — the Rust bridge (no egui).
- Delete `client_view.rs`, `host_view.rs`, `jobs_panel.rs`, `pairing_dialog.rs`, `settings.rs`,
  `tray.rs` once their behavior lives in the Slint UI + bridge.

### 7.3 Slint view model (suggested)

Window root properties (`in-out`), all set from Rust:

Global/status:
`page: int` (0 Host, 1 Client, 2 Settings), `dark: bool`, `hosting: bool`, `host-running: bool`,
`host-status: string`, `version: string`, `host-error: string`, `firewall-configured: bool`,
`firewall-busy: bool`.

Drafts (Rust sets **only** on load/after save; never in the periodic sync, or typing gets
overwritten):
`host-name-draft`, `host-port-draft`, `pin-draft`, `require-pin`, `autostart`,
`relay-base-draft`, `outbox-draft`, `manual-addr`.

Static/diagnostic:
`elevated`, `protected-mode`, `host-id`, `fingerprint`, `data-dir`, `pin-set`.

Models (use `slint::ModelRc` + `Rc<VecModel<T>>`, `set_vec` on change):
- `printers: [PrinterRow]` — name, driver, status, jobs, online, shareable, shared,
  display-name, spooler-name.
- `paired: [PairedRow]` — id (string), name, paired-age, seen-age, revoked.
- `hosts: [HostRow]` — id (string), name, addr, pin-required.
- `host-printers: [HostPrinterRow]` — host-id, printer-id, name, model, status, online, saved.
  Render nested with `for host in hosts { for p in host-printers { if p.host-id == host.id ... } }`
  (small data sets; avoids nested models in structs).
- `saved: [SavedRow]` — id, queue, model, host, addr, relay-status, relay-ok, relay-bad, last-job,
  show-offline-note.
- `host-jobs: [JobRow]`, `client-jobs: [JobRow]` — when, printer, peer, doc, size, pages, state,
  color, error.
- `notices: [NoticeRow]` — text, level.

Pairing modal:
`pairing: bool`, `pair-title`, `pair-host`, `pair-fingerprint`, `pair-needs-pin`, `pair-pin`,
`pair-error`, `pair-busy`, plus internal host/printer ids kept in Rust.

Callbacks (Rust handlers):
`set-hosting(bool)`, `save-host-settings()`, `set-require-pin(bool)`, `set-pin(string)`,
`clear-pin()`, `toggle-printer(string spooler, bool enable)`,
`rename-printer(string spooler, string name)` (fire on Enter, not per keystroke),
`refresh-printers()`, `setup-firewall()`, `revoke-client(string id)`, `find-host(string addr)`,
`begin-pair(string host-id, string printer-id)`, `install-printer()`, `cancel-pair()`,
`test-page(string saved-id)`, `remove-printer(string saved-id)`, `set-autostart(bool)`,
`save-network()`, `copy-diagnostics()`, `open-data-folder()`, `quit()`.
Window chrome: `drag-window()`, `minimize-window()`, `maximize-window()`, `hide-window()`.

Use action-label text for the sharing toggle if it is rendered as a button, but a real `Toggle`
switch is preferred to match WinUI3.

### 7.4 Rust bridge design

```rust
struct Bridge {
    shared: Arc<Shared>,
    ui: slint::Weak<AppWindow>,
    tray: AppTray,                 // separate generated handle
    tx: std::sync::mpsc::Sender<ActionDone>,
    rx: std::sync::mpsc::Receiver<ActionDone>,
    model_handles: ...             // Rc<VecModel<...>> per model
    last_fingerprint: u64,
}
```

- Callbacks: upgrade the weak window, read drafts/properties, mutate `shared.config()` and call
  `set_config` or spawn async work on `shared.runtime`; report completion via the `ActionDone`
  channel (same enum as today: Probed/Installed/Removed/Tested/Firewall).
- Timer: `slint::Timer::default().start(TimerMode::Repeated, Duration::from_millis(500), move || tick())`.
  On each tick:
  1. drain `ActionDone` and apply dialogs/notices/busy flags;
  2. poll `crate::ipc::take_quit()` → quit event loop; `take_activation()` → show + focus;
  3. update tray hosting label + tooltip from `cfg.host.enabled` and relay count;
  4. compute a cheap fingerprint of `CoreState` + relevant config (lengths, statuses, job states,
     notices count/first id, relay listening/errors, discovered addrs, host_running) and only
     rebuild models/properties when it changes. Do not touch draft text properties here.
- Showing the window: `ui.show()`, then Win32 `ShowWindow(hwnd, SW_RESTORE)` +
  `SetForegroundWindow(hwnd)`; re-apply DWM frame.
- Hide: `ui.hide()` (X), `on_close_requested(|| CloseRequestResponse::HideWindow)` for OS close.
- Quit: `slint::quit_event_loop()`; after `run_event_loop_until_quit()` returns, run the existing
  shutdown sequence.
- `--autostart`: create the window but do not `show()` it; the tray icon must be created first.
  `--background` can keep the current headless console path (no Slint, no tray) unchanged.
- Model updates must not clear scroll position; reuse the same `VecModel` and `set_vec`.
- Record/compare your own Rust row structs for equality before converting to Slint structs; Slint
  setters do not guarantee "no-change" suppression.

The Slint probe's Win32 helpers (`hwnd_of`, `apply_frame`, `is_dark`, `install_chrome`, `hit_test`)
were deleted with it; the equivalent egui-build helpers live in `src\win\`. If a minimum window size
or title-bar double-click maximize is ever added, implement it there.

### 7.5 Slint UI structure recommendation

Keep the probe's components (`Theme`, `Toggle`, `NavItem`, `CaptionButton`, `Card`) and add:
`SectionTitle`, `RowSeparator`, `Pill`/`Chip`, `StatusDot`, `Banner`, `ListRow`, `EmptyState`,
`PrimaryButton`, `SecondaryButton`, `Dialogs/PairingDialog`, `Overlay`.

Layout: window `VerticalLayout` → 40px title bar (`TouchArea` drag first, then HorizontalLayout
with icon/title/spacer/caption buttons) → `HorizontalLayout` (220px rail + ScrollView content with
32px padding and `max-width: 920px` centered or full width). Give cards `width: 100%` so they
stretch when maximized (the probe card did not stretch — fix that in the port).

WinUI3 styling tokens are already in the probe's `Theme` global:
light page `#f3f3f3`, pane `#ebebeb`, card `#ffffff`, text `#1b1b1b`, muted `#5d5d5d`,
line `#e5e5e5`, accent `#0067c0`; dark page `#202020`, pane `#272727`, card `#2d2d2d`,
text `#ffffff`, muted `#c7c7c7`, line `#3a3a3a`, accent `#60cdff`. 8px card radius, 4px controls.

Settings → About must include:

```slint
import { AboutSlint } from "std-widgets.slint";
// ...
AboutSlint { }
```

(Verify the exact import/name in the installed Slint version; if the widget is unavailable, use
the official "Built with Slint" badge image from the Slint package and link/credit in text. The
license choice must not ship without attribution.)

### 7.6 Acceptance criteria for the port

- All Host/Client/Settings/pairing/jobs/tray behaviors in §5 work, verified by real clicks, not
  just compilation.
- Window: custom chrome, drag, minimize, maximize (respects taskbar), close-to-tray, taskbar
  button, minimum size, edge resize, correct rendering after minimize/restore.
- Hidden start (`--autostart`) shows no window but tray works; launching the exe again shows it;
  tray Open shows it; tray Quit performs the full shutdown/pause sequence.
- System light/dark colors at startup; DWM rounded corners/dark frame.
- `cargo test` still passes; `--diagnose` unchanged; real Canon G2010 queue untouched.
- About screen contains Slint attribution.

---

## 8. Verification playbook (Windows)

- Build/test: `cargo build`, `cargo test`.
- State without UI: `cargo run -- --diagnose` (JSON; check `host`, `local_printers`,
  `saved_printers`, `relays`, `firewall_rules`, `protected_print_mode`).
- Queue check: `Get-Printer -Name "Canon G2010 series"` (status should be `Normal`), and target
  `"Canon G2010 series (peerPrint)"` when a client queue exists.
- Real UI clicks: the helper scripts were deleted in the 2026-11-10 cleanup. If needed, re-create
  them (bring the window to the foreground and require `foreground=True` before any click or
  capture; otherwise screenshots show whatever is on top).
- Tray: re-create the tray-menu helper if needed; click by menu-item text, and note tray icon order
  can change between restarts.
- Tray hidden/close test: hide the window, then use the tray menu item "Open peerPrint"; confirm
  the window is visible and rendered.
- Never claim a UI behavior is verified from a screenshot alone if the window might be occluded;
  check `foreground=True` and look for the window content.
- `--quit` test: run the app, then `cargo run -- --quit`; confirm process exits and the client
  queues are paused (the shutdown path).
- Autostart test: set the toggle in Settings, check the `Run` registry entry
  (`win::autostart`), then start with `--autostart` and confirm hidden start.
- Coordinate mapping: since the window is borderless, `PrintWindow(flag 2)` captures exactly the
  client area (1000×720 at the default size) and screen pixels = client pixels plus the window
  origin. Older notes mention a (8, 31) offset — that applied to the native-caption build and is
  obsolete. A 31 px error is enough to miss small chips; the 2026-11-10 history-filter "bug" was
  exactly that.
- Synthetic clicks can be lost once right after a tab switch/focus change (Windows foreground
  activation). Re-send the click before suspecting the widget; the chip debug log that proved this
  printed `clicked=true` on the second attempt.
- Notifications: trigger a notice (Set PIN with an empty field on Host is harmless), confirm the
  flyout floats bottom-left and is gone by ~5 s, the sidebar badge stays until the bell is opened,
  the center lists it, Clear empties it, and no flyout replays after an engine restart (notices are
  memory-only).
- Update check: use a scratch `PEERPRINT_DATA_DIR` and point `PEERPRINT_UPDATE_API` at a local
  HTTP server answering `302` with `Location: https://github.com/november-saiful/peerPrint/
  releases/tag/v<newer>` (or 404 for "up to date"). Confirm the flyout, the Settings → Updates card
  and **View release**; stop the server to confirm the error text. `updates.last_check` gates the
  daily check, so reset the scratch config (or start a new data dir) between runs. Relaunch the
  same data dir to confirm the second run makes no HTTP request, replays no flyout, and still shows
  the cached release (or the cached error) in Settings. Never publish a test release to the real
  repository for this.

---

## 9. Remaining work

### UI (done — egui retained)

- The Slint migration (#10–#14) was completed, verified, then reverted on 2026-11-09 for the
  licensing reason above. egui/eframe is the shipped UI; no further UI migration is planned.
- egui fixes made during the restore: shell/tray/IPC handling moved to `eframe::App::logic()` so it
  works while minimized/hidden; tray Open restores minimized windows via `ShowWindow(SW_RESTORE)`;
  notice banners use readable light text, wrap, and post only once per close.
- 2026-11-10 redesign: sidebar shell + `theme.rs` design system (see §4 cheat sheet). Views were
  rewritten around cards/list rows; the 8-column job table became responsive rows; the pairing
  window became `egui::Modal` with a fingerprint panel. No behavior, config, or protocol changes.
  Verified with a scratch data dir at 1000×720 and 820×600; `cargo build` clean for the UI code,
  `cargo test` 11/11. Not yet exercised: enabled-host card, populated job rows, pairing modal,
  dark mode (all need a printer/host or a system theme change).

### Job cancel (done 2026-11-10)

- Client jobs: `client::cancel_client_job` removes the outbox `.json`/`.bin`, marks the record
  `Cancelled`, appends it to `jobs.jsonl`, and sets a cancel flag. If an upload is in progress the
  send loop sees the flag within one 32 KB chunk, sends `ClientMsg::JobCancel`, and the host aborts
  the transfer and records `Cancelled` (not `Failed`).
- Host jobs: `Shared::request_cancel` + `host.rs::track_spool_job` polls every 500 ms and calls
  `spooler::cancel_job`, then records `Cancelled` and replies with `ErrCode::Cancelled`; the
  uploading client maps that code to its own `Cancelled` state (not `Failed`).
- UI: a Cancel button appears in the Job history table for host jobs in Receiving/Spooling/Printing
  and client jobs in Queued/Waiting for host/Uploading/Printing. For a client job already printing,
  pressing it sets the cancel flag that the still-open upload loop turns into `ClientMsg::JobCancel`,
  and the host stops the spool job.
- Verified: queued client cancel end-to-end against a scratch data dir (banner, table state
  `Cancelled`, outbox directory empty, `jobs.jsonl` appended). Printing-job cancel verified in the
  UI with a scratch fixture: the banner "Asking the host to cancel '…'…" appears and the row stays
  `Printing` because no host exists to acknowledge. Host cancel and the remote network path are
  reviewed + unit-tested only (no shared printer on this PC).

### Remote cancel while printing (done 2026-11-10)

- Problem: during spool tracking the host never read the socket, and the old `read_msg` used
  `read_exact`, which is not cancel-safe — a timer racing it could swallow a partial TLS frame.
- `net\framing.rs`: new `MsgReader<R>` keeps a persistent buffer and appends via `read()` inside a
  loop, parsing only complete 4-byte-length + postcard frames. Dropping a pending `read()` future is
  safe; bytes already received stay in the buffer (rustls keeps its record state). Unit tests:
  `buffered_reader_decodes_back_to_back_frames` and `buffered_reader_survives_a_cancelled_read`
  (writes half a frame, times out the read, writes the rest, decodes successfully).
- `host.rs::track_spool_job` is now generic over `AsyncRead + AsyncWrite` and runs a
  `tokio::select!` between the 500 ms poll tick and `reader.read::<ClientMsg>()`. `JobCancel` for the
  matching `tx` cancels the spool job and returns `TrackOutcome::Cancelled { by_client: true }`;
  `Ping` is answered; a disconnect just stops listening and printing continues to completion; all
  other messages are ignored. Local UI cancel still works through `Shared::request_cancel` and
  reports `by_client: false`. `tokio::io::split(&mut *stream)` provides the halves.
- `client.rs::upload_job` splits the stream after `JobEnd`, runs a 1 s tick that sends `JobCancel`
  once when `shared.cancel_requested(job_ref)` is set, and maps the host's `ErrCode::Cancelled` to
  `UploadOutcome::Cancelled` (never `Failed`). `cancel_client_job` in `Printing` state sets the
  cancel flag and shows "Asking the host to cancel …" instead of editing the outbox.
- Final `JobDone`/`JobFailed` replies are written with `let _ =` so a client that vanished mid-print
  cannot turn a completed job into a failed one.
- Not exercised end-to-end: needs a real host with a shared printer (or the user's second PC).

### Retry / dedupe (reviewed 2026-11-10)

- Offline retry: `client.rs::process_outbox` re-runs every 5 s; failures keep the outbox entry,
  increment `attempts`, set `next_try` via `backoff_secs` (1,2,4,8,16,30 s cap + 0–2 s jitter), and
  show `Waiting for host`. Entries survive restarts (JSON + `.bin` under
  `%LOCALAPPDATA%\PeerPrint\outbox\<saved-id>\`). Verified by code review and a new backoff unit
  test; not exercised against an unreachable-then-recovered host without the second PC.
- Dedupe: `host.rs::handle_job` checks `shared.recent_host_job(client_job_ref)` before accepting a
  transfer, replays the prior `JobDone`/`JobFailed`, and an `inflight` set rejects concurrent
  duplicate transfers with `Busy`/retryable. `remember_host_job` keeps 24 h. Verified by review.

### Verification log (2026-11-10, final tree)

Release binary: `target\release\peerprint.exe`, 12,187,648 bytes (built 2026-11-10 00:53, after the
remote-cancel changes; `cargo build --release`, 11 pre-existing dead-code warnings, no new ones).
`target\` and therefore that binary were deleted in the 2026-11-10 cleanup; run `cargo build
--release` to reproduce it.

```text
cargo test
  running 11 tests
  client::tests::retry_backoff_grows_and_is_capped ... ok
  config::tests::queue_names_are_sanitized_for_windows ... ok
  logging::tests::redacts_secret_assignments ... ok
  pairing::tests::pin_limiter_escalates_and_resets ... ok
  config::tests::install_spec_validation_rejects_unsafe_fields ... ok
  config::tests::token_hash_verification_is_client_specific ... ok
  net::framing::tests::buffered_reader_decodes_back_to_back_frames ... ok
  net::framing::tests::buffered_reader_survives_a_cancelled_read ... ok
  net::framing::tests::roundtrips_messages_and_rejects_oversized_frames ... ok
  pairing::tests::pinned_client_rejects_a_different_certificate ... ok
  pairing::tests::pairs_with_correct_pin_and_rejects_wrong_pin ... ok
  test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Memory sample = fresh launch, stock data dir (`%LOCALAPPDATA%\PeerPrint`), one window, Host tab,
idle (no discovery/pairing activity):

| Sample | Working set | Private bytes | Threads | Handles |
|---|---|---|---|---|
| 6 s after launch | 97.6 MB | 53.0 MB | 22 | 354 |
| 26 s after launch (idle) | 95.6 MB | 51.2 MB | 22 | 358 |

Reference points from earlier sessions: egui **debug** 107.9 MB WS / 53.4 MB private; archived
Slint **debug** build 99 MB WS / 41.3 MB private. Interesting consequence: egui release is only
slightly below egui debug on working set (GPU/driver and runtime dominate), and Slint's private
bytes were lower. Do not claim egui is the lightest based on these numbers.

Other checks run on this tree: `--diagnose` returns honest empty state (`local_printers: []`,
`running: false`); the rebuilt release starts, paints the window, and quits via the control pipe;
queued-client cancel verified end-to-end earlier against a scratch data dir; printing-client cancel
verified in the UI with a scratch fixture (Cancel button, "Asking the host to cancel…" banner, row
stays `Printing` without a host; fixture and screenshots deleted);
the redesigned shell was checked visually at 1000×720 and 820×600 (Host, Client, Settings; dark
mode not exercised); the borderless custom title bar was checked on the light theme (title-bar drag
moves, double-click maximizes/restores, edge drag resizes, minimize/close buttons work, close hides
to tray with the process alive, taskbar title and icon slots restored after the earlier
blank-caption regression; scratch data dir and the test-written `HKCU\...\Run\PeerPrint` value were
removed afterwards);
`cargo fmt --check` is **not** clean on the pre-existing tree, so do not run `cargo fmt` wholesale.

### Still open

- Remote cancel while printing: shipped, but the host↔client exchange only runs for real with a
  shared printer on the host side. Re-verify on the two-PC pass: start a print, press Cancel on the
  client, expect host `Cancelled` + client `Cancelled` (not `Failed`).
- Two-PC discovery/pairing/print — requires the user's second PC; do not simulate.
- Full `--uninstall-all` — destructive; requires explicit user approval.
- Installer stays unverified for two paths: the interactive wizard (silent `/ALLUSERS` used the
  default role) and Just me / per-user install mode. All users install + uninstall are verified.
- The tray-menu portion of #9 is complete and verified.

---

## 10. Hard rules / gotchas

- Do not run `--uninstall-all` or remove printers without explicit approval.
- Do not touch the user's real "Canon G2010 series" queue; only the peerPrint-created queues are
  ours.
- Do not invent demo printers/hosts/jobs. Empty states must be honest.
- Slint is not part of the app. Do not re-add Slint without resolving the Royalty-free attribution
  requirement with the user first.
- Keep the service layer intact; the UI rewrite must not change network/protocol/spooler logic.
- `cfg.host.enabled` drives the tray label; `host_running` only drives the status dot/text.
- The host only runs when at least one printer is shared — do not "fix" this; it is intentional.
- The config file is saved atomically via `AppConfig::save`; always save through
  `shared.set_config`.
- `refresh_local_printers()` is blocking; do not call it on every timer tick. Call it at startup,
  on Refresh, and maybe every 15–30s if needed.
- Pairing/install/remove/test/firewall are async/blocking-elevation flows; keep the busy flags and
  failure notices that exist today.
- `pin_dpapi` presence means a PIN is set; the PIN itself is never available for display.
- Text fields must be drafts (see §7.3) or the periodic model sync will overwrite the user's typing.
- Windows scripting/foreground pitfalls in §6 apply to the real app too; the window title is
  "peerPrint".

---

## 11. Key paths quick reference

```
<root>\Cargo.toml                     dependencies (egui/eframe 0.37, tray-icon, raw-window-handle)
<root>\Cargo.lock                     committed lockfile
<root>\build.rs                       embeds assets\peerprint.rc (icon + manifest)
<root>\src\main.rs                    CLI + lifecycle + eframe startup
<root>\src\service.rs                 Shared/CoreState (UI data source), cancel flags, dedupe cache
<root>\src\config.rs                  AppConfig/HostConfig/ClientConfig
<root>\src\client.rs                  relay/outbox/upload, cancel_client_job, backoff
<root>\src\host.rs                    host service, transfer, spool tracking, cancel polling
<root>\src\ui\mod.rs                  egui shell (sidebar/content/notices/shell logic)
<root>\src\ui\theme.rs                 palette, fonts, styles, cards, pills, buttons, toggle, modal frame
<root>\src\ui\host_view.rs            host page
<root>\src\ui\client_view.rs          client page
<root>\src\ui\settings.rs             settings/diagnostics
<root>\src\ui\pairing_dialog.rs       pairing modal
<root>\src\ui\jobs_panel.rs           job rows + Cancel button
<root>\src\ui\tray.rs                 tray-icon crate
<root>\installer\peerprint.iss        Inno Setup package script
<root>\assets\peerprint-256.png       runtime window icon
```

(The `.tmp\` scratch tree, archived Slint UI, probe project and `target\` were deleted on
2026-11-10.)

---

## 12. Historical: suggested actions from the Slint era (do not execute)

These steps were written while the Slint migration was planned; the migration was reverted on
2026-11-09. They are kept only for reference.

1. `cargo build && cargo test` — confirm the environment (MSVC) works.
2. ~~Run the Slint probe and finish probe validation~~ (probe deleted 2026-11-10).
3. ~~Port the shell to `ui/app.slint`~~ (reverted; egui retained).
4. ~~Port pages to Slint~~ (obsolete).
5. ~~Remove egui~~ (never; egui is the shipped UI).
6. Work from §9 instead.
