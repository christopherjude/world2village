# Village (world2village)

A friendly Windows GUI wrapper around [n2n](https://github.com/ntop/n2n)'s `edge.exe`, built so non-technical friends can join a private virtual LAN with a single invite code — no community/key/supernode fields, no driver hunting, no CLI flags. Originally built to let a small group play *Command & Conquer: Generals* together online.

## What this is (and isn't)

- This project does **not** reimplement the n2n protocol. It shells out to a compiled `edge.exe` (and optionally talks to a `supernode.exe`) as a subprocess and manages it.
- The value-add is entirely in the wrapper: a simple invite-code UX, a modern UI, automated driver setup, and status feedback a non-IT person can actually understand.

## Stack

- **Rust + Tauri** for the app shell — small binary, native subprocess/service management from Rust, HTML/CSS/JS frontend so the UI doesn't look like a Win32 dialog.
- **Windows-only** target for v1 (matches the gaming use case — Generals is Windows-only anyway).
- **tap-windows6** driver (OpenVPN's, from https://github.com/OpenVPN/tap-windows6 — n2n's own docs point here). **WinTun was considered and rejected — see gotcha below, do not revisit without re-reading it.**
- License: MIT.

## Product decisions already made

- **Invite code, not raw fields.** Users never see community/key/supernode host:port as separate inputs. One code/link encodes all three; pasting it is the entire "join" flow.
- **Never expose `-a` mode picking to the user.** Always let the supernode assign the IP (see gotcha below) unless a future power-user "advanced" mode explicitly asks for a static IP.
- **Advanced settings are collapsed by default.** MTU, compression, header encryption, custom params — hidden unless a user opts into "Advanced."
- **Big, obvious connection state** + the assigned overlay IP shown large and copyable, with a one-line hint ("Your community IP is").

## Hard-won gotchas — do not relearn these the hard way

- **`-a dhcp` is a trap.** It does NOT mean "ask the supernode for an IP." In n2n's `edge.c`, `-a dhcp` selects `TUNTAP_IP_MODE_DHCP` — "obtain IP from other edge DHCP services," i.e. it waits for *another peer* on the overlay to run a real DHCP server. It will hang forever with no such peer. **Omitting `-a` entirely** triggers `TUNTAP_IP_MODE_SN_ASSIGN` ("automatically assign IP address by supernode"), which is what we actually want. Village's subprocess-arg builder must never emit `-a dhcp`.
- **The supernode needs an IP pool.** `supernode.exe` must be started with `-a <subnet>/<cidr>` (e.g. `-a 10.100.0.0/24`) or it has nothing to hand out even when the edge asks correctly.
- **Windows requires elevation to configure adapter IPs.** A plain unelevated `edge.exe` opens the TAP/WinTun device fine but silently fails to set its IP. Solve this architecturally — e.g. install as a Windows Service (SYSTEM-elevated) rather than asking users to "run as Administrator," which non-IT friends will get wrong or find confusing.
- **Distinct MAC addresses matter.** Two edges connecting with identical/default MACs collide at the supernode ("authentication error, MAC or IP address already in use"). Village should generate/persist a stable per-install MAC (or rely on n2n's own generation) — never hardcode the same MAC across installs.
- **WinTun does NOT work with n2n — don't "fix" this again.** WinTun is a layer-3 (IP-only) adapter; n2n is a layer-2 (Ethernet) system. Straight from the n2n maintainers (github.com/ntop/n2n#1086): "WinTun provides a layer-3 virtual network card, however N2N is a layer-2 system... nobody has [written a layer-2 emulation]." There is zero WinTun code anywhere in upstream n2n. This also matters functionally, not just technically: old RTS LAN discovery (Generals included) typically depends on Ethernet-level broadcast, which a layer-3-only adapter can't carry. Village must use the real tap-windows6 driver.
- **The tap-windows6 install dance is real, and Village automates it — the user never sees it.** The driver (`OemVista.inf` + `tap0901.cat` + `tap0901.sys` + `devcon.exe`, per-arch, from OpenVPN's tap-windows6 releases) is installed with `devcon.exe install OemVista.inf tap0901` (hardware ID `tap0901`). This runs once, elevated, as part of the same one-time Windows Service install/setup step — bundled alongside `edge.exe` in `bin/`, copied to `%ProgramData%\Village\bin\` at install time, invoked by `village-service`'s installer. One UAC prompt covers both the service registration and the driver install; nothing after that requires elevation or user driver-hunting.

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

v1 builds end-to-end and produces real installers (`dist/Village_0.1.0_x64_en-US.msi`, `dist/Village_0.1.0_x64-setup.exe`), verified with a real `cargo tauri build` on a Windows host (not just `cargo check` — this repo lives in WSL2, so builds are done by copying to a native Windows path, e.g. `C:\dev\world2village`, and running cargo/tauri-cli there; `powershell.exe` is reachable directly from WSL bash for this).

Client-only in scope: Village never launches `supernode.exe` — supernodes are hosted separately, out-of-band. The app manages a list of saved "Server" profiles (nickname + community + key + supernode host:port), not a single paste-and-connect flow. Invite codes export/import one profile; a hidden Advanced screen lets the host type raw fields and generate codes for friends.

Still pending, not yet done:
- Click-testing the actual GUI on Windows (build success confirmed; UX flow not yet exercised end-to-end by a human).
- Verifying `edge.exe`'s real stdout format matches `village-service`'s IP-parsing guess (written defensively, but unverified against real output).
- A `village-security` pass over the driver-install/subprocess/IPC code (several rounds of it were explicitly skipped this session to prioritize getting a working build).
- Pinning exact `edge.exe` version/hash in `bin/README.md` (currently records whatever build was on hand, not a specific upstream release tag).
