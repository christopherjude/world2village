# bin/

This directory holds externally-supplied binaries that Village shells out to
(or installs as a driver) at runtime. It does **not** contain Village's own
source, and nothing in it is vendored or redistributed by this repository —
per `CLAUDE.md`, Village shells out to a compiled `edge.exe` and installs the
tap-windows6 driver but does not reimplement or redistribute either. You
must obtain these files yourself from upstream releases and place them here
before building/running the Windows app or service.

These files are gitignored (`bin/*.exe`, `bin/tap-driver/`) — do not commit
them.

## Required files

### `edge.exe`

The n2n edge client, built from or downloaded from an official release of
upstream n2n: https://github.com/ntop/n2n

- Version: whatever build the project owner supplied (not pinned to a specific
  upstream release tag yet — TODO once we standardize on one).
- SHA256: `382dff12c2e0201f97bfcf01500b8437b676a7852e201310928066bbd67c23b0`

### `bin/tap-driver/` (tap-windows6)

The tap-windows6 driver files, needed because WinTun does not work with n2n
(layer-3 vs n2n's layer-2 design — see `CLAUDE.md`'s gotchas). Sourced from
the `amd64/` folder inside the `dist.win10.zip` asset of an OpenVPN/
tap-windows6 GitHub release: https://github.com/OpenVPN/tap-windows6/releases

Place these four files directly in `bin/tap-driver/`:

- `bin/tap-driver/devcon.exe` — the install tool
- `bin/tap-driver/OemVista.inf` — driver INF
- `bin/tap-driver/tap0901.cat` — catalog/signature
- `bin/tap-driver/tap0901.sys` — driver binary

- Release version: `9.27.0` (https://github.com/OpenVPN/tap-windows6/releases/tag/9.27.0)
- SHA256 (per file):
  - `devcon.exe`: `bee3a63db18565ab77ad5714594b658c1d47c7e475009b25d430df9ed634ea46`
  - `OemVista.inf`: `1327ab3a8c50691f04bea8e2ca356c5b604092a719e219464f8cc4b42e192de9`
  - `tap0901.cat`: `ee062e5ef2743ceab10c64830e4cefe52e35cc1ece85947ac4e61ddd1c0b05f7`
  - `tap0901.sys`: `581dcaace05d5c1ac9512457ff50565aca5d904d2c209bd3fc369ca4d4a0d2b1`

## Why pin a version/hash at all

Recording the expected version and SHA256 here lets a future install/build
step verify the binaries dropped into this directory are the ones Village
was actually tested against, rather than silently trusting whatever a
contributor happened to have lying around. Fill in the TODOs once a specific
n2n/tap-windows6 release has been chosen and verified.
