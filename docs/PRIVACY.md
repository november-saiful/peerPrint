# Privacy

peerPrint runs on your own PCs and your local network. It has no accounts, no telemetry and no
analytics, and it never sends printer jobs, documents or job history to a third party.

## What leaves this PC

- **Update checks (optional, once a day).** The app requests the public releases page at
  `github.com/november-saiful/peerPrint/releases/latest` and reads its redirect to the newest
  stable tag, with `User-Agent: peerPrint/<version>`. GitHub receives the usual connection
  metadata (IP address, TLS handshake, headers); no device or user identifier is added. Turn it
  off in **Settings → Updates**.
- **Local discovery.** mDNS/DNS-SD advertisements and browsing happen on the local network only and
  expose the display name you choose. Nothing is sent to the internet.
- **Printing.** Jobs travel directly from the client PC to the host PC over TLS, authenticated with
  a PIN-paired token. There is no cloud relay.

## What is stored on this PC

Under `%LOCALAPPDATA%\PeerPrint` (single-user install) or `%ProgramData%\peerPrint` (all-users
install):

- `config.json` — ports, role, update-check state, the local host name you chose.
- `identity/` — the host certificate and its DPAPI-protected private key (bound to this Windows
  user or machine).
- `paired_clients.json` — client names, tokens (hashed) and certificate fingerprints.
- `outbox/`, `state/` — queued jobs and queue bookkeeping, deleted as jobs complete.
- `logs/` — rotating local logs that redact secrets.

Uninstalling removes the queues and service; delete the data folder to remove the rest.

## Contact

Questions or data-removal requests: open an issue at
<https://github.com/november-saiful/peerPrint/issues>.
