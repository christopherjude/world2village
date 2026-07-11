# Village (world2village)

A friendly Windows GUI wrapper around [n2n](https://github.com/ntop/n2n)'s `edge.exe`, built so non-technical friends can join a private virtual LAN with a single invite code — no community/key/supernode fields, no driver hunting, no CLI flags. Originally built to let a small group play *Command & Conquer: Generals* together online.

## What this is (and isn't)

- This project does **not** reimplement the n2n protocol. It shells out to a compiled `edge.exe` (and optionally talks to a `supernode.exe`) as a subprocess and manages it.
- The value-add is entirely in the wrapper: a simple invite-code UX, a modern UI, automated driver setup, and status feedback a non-IT person can actually understand.

## Stack

- **Rust + Tauri** for the app shell — small binary, native subprocess/service management from Rust, HTML/CSS/JS frontend so the UI doesn't look like a Win32 dialog.
- **Windows-only** target for v1 (matches the gaming use case — Generals is Windows-only anyway).
- **WinTun** driver (not the legacy tap-windows6/NDIS5/6 driver n2n normally bundles) — one universal driver, no per-arch/per-OS-version installer matrix, far fewer "adapter didn't show up" failure modes.
- License: MIT.

## Product decisions already made

- **Invite code, not raw fields.** Users never see community/key/supernode host:port as separate inputs. One code/link encodes all three; pasting it is the entire "join" flow.
- **Never expose `-a` mode picking to the user.** Always let the supernode assign the IP (see gotcha below) unless a future power-user "advanced" mode explicitly asks for a static IP.
- **Advanced settings are collapsed by default.** MTU, compression, header encryption, custom params — hidden unless a user opts into "Advanced."
- **Big, obvious connection state** + the assigned overlay IP shown large and copyable, with a one-line hint ("paste this into Generals' LAN IP field").

## Hard-won gotchas — do not relearn these the hard way

- **`-a dhcp` is a trap.** It does NOT mean "ask the supernode for an IP." In n2n's `edge.c`, `-a dhcp` selects `TUNTAP_IP_MODE_DHCP` — "obtain IP from other edge DHCP services," i.e. it waits for *another peer* on the overlay to run a real DHCP server. It will hang forever with no such peer. **Omitting `-a` entirely** triggers `TUNTAP_IP_MODE_SN_ASSIGN` ("automatically assign IP address by supernode"), which is what we actually want. Village's subprocess-arg builder must never emit `-a dhcp`.
- **The supernode needs an IP pool.** `supernode.exe` must be started with `-a <subnet>/<cidr>` (e.g. `-a 10.100.0.0/24`) or it has nothing to hand out even when the edge asks correctly.
- **Windows requires elevation to configure adapter IPs.** A plain unelevated `edge.exe` opens the TAP/WinTun device fine but silently fails to set its IP. Solve this architecturally — e.g. install as a Windows Service (SYSTEM-elevated) rather than asking users to "run as Administrator," which non-IT friends will get wrong or find confusing.
- **Distinct MAC addresses matter.** Two edges connecting with identical/default MACs collide at the supernode ("authentication error, MAC or IP address already in use"). Village should generate/persist a stable per-install MAC (or rely on n2n's own generation) — never hardcode the same MAC across installs.
- **The legacy driver install dance is real pain.** The old tap-windows6 driver needs `tapinstall.exe find/remove/install` per NDIS version (5 vs 6) and per arch (x86/x64/ARM/WinXP). This is exactly what WinTun avoids — prefer it.

## Reference material this project was informed by

- Upstream engine: https://github.com/ntop/n2n (GPLv3 — Village only invokes its compiled binaries as a subprocess, it does not link against or redistribute modified n2n source, so Village's own MIT license is unaffected).
- Prior-art wrapper reviewed during design, for UX/packaging ideas only (not code reuse): https://github.com/happynclient/happynwindows

## Working with sub-agents in this repo

To keep the main conversation focused on planning and review rather than raw tool output, delegate work to the specialized sub-agents below instead of doing it inline:

| Agent | Use for |
|---|---|
| `village-coder` | Writing/editing Rust, Tauri, and frontend code — new features, bug fixes, refactors |
| `village-git` | All git operations: staging, committing, branching, diff review |
| `village-packages` | Adding/removing/updating Cargo and npm dependencies, lockfile hygiene |
| `village-builder` | Running builds (`cargo check`/`build`, `tauri build`), diagnosing compile errors |
| `village-security` | Reviewing subprocess arg construction, invite-code parsing, config/credential storage, and dependency vulnerabilities |

Default flow for a feature: `village-coder` implements → `village-builder` verifies it compiles → `village-security` reviews if it touches subprocess args/config/credentials → `village-git` commits.

## Status

Scaffolding only. No Cargo/Tauri project has been initialized yet — that's the first task for `village-coder` in the next session (`cargo install tauri-cli` if needed, `cargo tauri init`, pick a frontend approach).
