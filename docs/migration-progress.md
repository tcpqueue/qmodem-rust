# Migration progress

Target: OpenWrt 24.10 and later; Quectel and TD Tech MT5700 only.
This log supplements the original inventory. Entries below are implementation progress,
not a claim of complete migration or hardware compatibility.

## Added after the initial audit

- Native sysfs USB/PCIe inventory, driver/interface filtering, PCM exclusion, bounded ATI
  probes, model query fallbacks and restricted option-driver registration.
- Optional periodic device discovery and live TOML modem registration/configuration CRUD.
- Idle-port close/reopen, closed-handle rejection and stream disconnect notification.
  No automatic retry of potentially state-changing AT transactions.
- Quectel neighbour queries, platform-specific cell lock/unlock and traffic-counter
  query/save/reset. MT5700 explicitly reports unavailable modem counters.
- Cached, coalesced native status transactions with conditional SIM/operator/ICCID/
  serving-cell/CA queries; Quectel and MT5700 field decoding. Further fixture coverage
  and legacy edge-case comparisons are still needed.
- Native GSM7/UCS2 SMS encoding/decoding, concatenated SMS, SQLite schema v2,
  idempotent sends, ordered multipart import, pagination/conversations/read/delete,
  SIM storage listing and verified-index deletion, manual/poll/firmware-gated URC modes.
  Delivery reports, forwarding, legacy import and extended PDU cases remain outstanding.
- Native AT dialing for supported vendor branches plus netifd dynamic-interface plans
  for DHCP/QMI/MBIM. SIM switching now includes redial and reports separate switch/
  redial failure. MT5700 software-state writes occur before serial open, as upstream.
- Embedded device discovery, modem status/settings, network settings, neighbour/lock,
  traffic, SMS and AT-debug pages in addition to queue monitoring. Mobile navigation
  remains accessible instead of hiding all page links.

## Still required

- Complete network lifecycle validation, MHI-specific QMI/MBIM support, firewall4,
  bridge passthrough and restoration, IPv6/DNS edge cases, GPIO/LED and 5G Ethernet.
- Persistent boot initialization/cell locks, watchdog and traffic-history scheduling.
- SMS delivery reports/forwarding/retry/legacy import, URC loss and long-message expiry.
- Full original-function inventory reconciliation, OpenWrt SDK/cross builds and package
  installation/ACL tests. Actual USB/PCIe device tests wait for user-provided hardware.

## Verification

77 Rust tests pass, including kernel PTY serial/SMS tests and SQLite persistence.
Strict clippy passed before the latest UI-only additions. Embedded frontend build passes.
Browser checked device-page navigation, nullable status and native AT responses using
explicitly labelled PTY simulators. These checks are not real modem or router tests.
