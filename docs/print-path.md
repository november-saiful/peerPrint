# Windows print path — spike findings

Validated on: Windows 11 22631, x64, Canon G2010 series on USB001 (driver "Canon G2010 series", Windows x64).

## What the driver sends to the port

Printing a test page through a TCP/IP queue pointing at `127.0.0.1:39160` produced a **96 KB XML command stream** (Canon `ivec`/`vcn` namespaces, `StartJob` … ), not EMF. The spooler's print processor renders the document and hands RAW driver data to the port monitor, so a relay only ever sees finished driver output.

- Two prints of the identical document differed in **31 bytes**, all inside the driver-generated `job_description` GUID. The rendered payload is deterministic.
- Consequence: byte-for-byte fidelity of the relay is meaningful; compare SHA-256 to detect transport corruption, not to compare two separate driver renders.

## Queue creation

- `Add-PrinterPort -Name … -PrinterHostAddress 127.0.0.1 -PortNumber N` works on this spooler. **Do not pass `-SNMP:$false`**: Windows PowerShell 5.1 rejects the switch value with `One or more specified parameters for this operation has an invalid value`. SNMP is off when the switch is omitted. This was a real installer bug, now fixed.
- The port appears synchronously for loopback TCP/IP; still poll `Get-PrinterPort` for a few hundred ms before `Add-Printer`.
- `Add-Printer -DriverName 'Canon G2010 series'` works when the driver is already staged.
- **Removal**: `Remove-PrinterPort` can fail with `0x80070709` even when the queue is gone. `cscript prnport.vbs -d -r <port>` (under `%WINDIR%\System32\Printing_Admin_Scripts\*\`) succeeds; the uninstaller falls back to it.
- Bidirectional support must be cleared afterwards (`Win32_Printer.EnableBIDI = false` or `SetPrinterW` with `Attributes &= ~PRINTER_ATTRIBUTE_ENABLE_BIDI`).

## Replay and physical output

- `StartDocPrinterW(datatype="RAW")` + `WritePrinter` + `EndDocPrinter` on the Canon USB queue accepted the captured stream unchanged (96,420 bytes, SHA-256 `f3988b00…0aab`) and printed the page.
- Host spooler reported `PrinterStatus: Normal`, `JobCount: 0` after both jobs finished; no language monitor re-interpreted the bytes.
- No shadow queue is required for this model: the existing USB queue with BIDI off is byte-transparent. Keep the shadow-queue fallback for vendor drivers that add a language monitor/print processor on the host.

## Decisions

1. **Existing queue** is used by default; `clear_bidi` runs after every install.
2. **Shadow queue** remains a per-model fallback only when a spike shows a host-side transform.
3. Relay must spool to disk before uploading — satisfied and smoke-tested (outbox kept a 100 KB job across a process restart and retried with backoff against an unreachable host).
4. Queue names are sanitized; port names stay `PeerPrint_<id>`.

## Evidence

The raw captures (`.tmp/m1/captured.bin`, `fidelity-1.bin`, `fidelity-2.bin`, `setup.log`) were
deleted in the 2026-11-10 cleanup; the measured findings above are the recorded evidence. Re-capture
by printing a test page through a loopback TCP/IP queue if they are ever needed again.

## Compatibility

| Printer | Host queue | Driver | Status |
|---|---|---|---|
| Canon G2010 series (USB001) | existing queue, BIDI off | Canon G2010 series, Windows x64 | Verified: capture/replay byte-identical, client relay job (619,012 bytes) printed 1 page |

Requirements and caveats:

- The client needs the same printer driver as the host. peerPrint forwards RAW, driver-rendered data and never substitutes a text-only driver.
- The host queue must stay BIDI-off; peerPrint clears `PRINTER_ATTRIBUTE_ENABLE_BIDI` after install.
- Windows protected print mode (`HKLM\SOFTWARE\Policies\Microsoft\Windows NT\Printers\WPP`, `WindowsProtectedPrintMode=1`) blocks third-party v3 drivers. peerPrint reports it in `--diagnose` and Settings → Diagnostics.
- A model whose vendor language monitor changes bytes on the host needs the shadow-queue fallback described above.
