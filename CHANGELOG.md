# Changelog

All notable changes to peerPrint are recorded here. Releases use
[Semantic Versioning](https://semver.org/); the app itself reports the same version in
Settings and on the command line.

## [Unreleased]

### Fixed

- Manual two-sided printing from client queues. Windows' `Add-Printer` defaults to the `winprint`
  print processor, which skipped the driver's manual-duplex handling, so "print on both sides"
  printed one side per sheet with no flip prompt. Client queues now adopt the print processor the
  driver installs (for example Canon's G2010 processor). Existing queues are flagged in Client and
  can be switched in place with **Repair queue**, without re-pairing.

### Added

- `--diagnose` reports each queue's print processor and whether it needs repair.

## [0.1.1] - 2026-11-10

### Changed

- Update checks follow GitHub's `releases/latest` redirect instead of the REST API, which is
  capped at 60 anonymous requests per hour per IP and failed with HTTP 403 on shared connections.

### Added

- Update checks against GitHub Releases: peerPrint asks once a day whether a newer stable release
  exists, shows a notification, and links to the release page from Settings → Updates. Nothing is
  downloaded or executed, and the check can be disabled.

## [0.1.0] - 2026-11-10

First public source release.

### Added

- Host/client printing over the LAN: the host shares USB Windows queues, clients install a normal
  Windows print queue backed by a loopback TCP/IP port and relay.
- mDNS discovery (`_peerprint._tcp.local.`) with manual `IP:port` fallback.
- PIN pairing (SPAKE2 bound to the TLS exporter) and per-client tokens; certificate pinning.
- TLS with a self-signed P-256 host identity; the private key and tokens are DPAPI-protected.
- Durable client outbox that buffers jobs while the host is unreachable.
- Job history (Accepted → Printing → Printed / Failed) with direction filters.
- Notification area app with flyouts, a notification center, close-to-tray and autostart.
- Per-user and all-users install modes; the all-users mode runs a Windows service before sign-in.
- Windows Firewall helper, protected-print-mode detection and JSON diagnostics (`--diagnose`).
- Custom borderless window with a unified title bar and caption buttons.
