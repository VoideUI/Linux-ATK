# Linux-ATK

The ATK protocol layer (command format, `EEPROMAddress`, HID report
handling) is provided by the [`libatk-rs`](https://crates.io/crates/libatk-rs)
crate, used here as a regular dependency. The DPI-specific encoding
formula (see below) was derived separately, by reverse-engineering real
HID traffic — see Acknowledgements.

## DPI protocol

The mouse stores 8 independent DPI profiles (DPI1..DPI8), grouped in
pairs across 4 EEPROM addresses:

| Address | EEPROMAddress | Profiles |
|---|---|---|
| 0x0c | DpiPair1 | DPI1, DPI2 |
| 0x14 | DpiPair3 | DPI3, DPI4 |
| 0x1c | DpiPair5 | DPI5, DPI6 |
| 0x24 | DpiPair7 | DPI7, DPI8 |

Each pair is 8 bytes: two identical 4-byte blocks back to back (one per
profile). Format of a single block:

```
idx    = DPI / 50 - 1                        (0-based index, step of 50 DPI)
idx_lo = idx & 0xFF
idx_hi = (idx >> 8) & 0xFF
byte0  = idx_lo
byte1  = idx_lo                              (duplicate of byte0)
byte2  = (idx_hi * 0x44) & 0xFF              (0x44 = 68, empirical constant)
byte3  = (0x55 - byte0 - byte1 - byte2) & 0xFF (checksum)
```

Writing uses `CommandId::SetEEPROM`, reading uses `CommandId::GetEEPROM`.
The `GetEEPROM` command requires the expected data length to be set
explicitly in the request — without it the device replies with status=1
(error) and empty data.

**Important:** when saving any single profile, ATK HUB resends all 4
DpiPair1/3/5/7 addresses at once (i.e. the entire set of 8 profiles),
even if only one was changed — the rest are echoed back unchanged. This
utility mirrors that behaviour: `set` first reads the whole pair
containing the target slot, modifies only the one 4-byte block, and
sends the pair back in full — so the other 7 profiles are never lost.

## Commands

- `Linux-ATK list` — finds connected ATK/VXE devices by HID VID
  (`0x373b`, `0x3554`).
- `Linux-ATK get` — reads and prints all 8 DPI profiles.
- `Linux-ATK set SLOT VALUE` — sets DPI for profile SLOT (1-8). `VALUE`
  must be a multiple of 50, in the range 100–30000.
- `Linux-ATK select SLOT` — switches the active profile to SLOT (1-8),
  without changing the DPI value itself — equivalent to clicking a DPI
  button in ATK HUB.

VID/PID and the working HID interface (usage_page/usage) are detected
automatically: the utility finds the device itself and tries known
vendor-specific interfaces until one responds. Manually specifying
`--vid`/`--pid`/`--usage-page`/`--usage` is only needed if
auto-detection fails on a particular model/firmware — see `Linux-ATK list`
for the actual values.

## Building

```bash
cargo build --release
```

On Linux, the `hidapi` `linux-shared-hidraw` backend is used, which
requires a system `hidapi` no older than 0.15 (the
`hid_send_output_report` symbol is missing in 0.14.x, which is still
found in some distributions, e.g. Ubuntu 24.04's apt repo). If the build
fails at the linking stage with an error about `hid_send_output_report`,
install a newer `hidapi` or switch to `features = ["linux-static-libusb"]`
in `Cargo.toml`.

Root-free device access on Linux requires udev rules, for example from
this gist: `https://gist.github.com/Speyll/b7803161b1ee43f258d484fe9e92c4b4`

## Usage

```bash
Linux-ATK list
Linux-ATK get
Linux-ATK set 1 1600
Linux-ATK select 1
```

If interface auto-detection fails (`Could not find a working HID
interface...`), run `Linux-ATK list`, find your mouse's vendor-specific
interface (usage_page is usually `0xffXX`), and pass it explicitly:

```bash
Linux-ATK --usage-page ff02 --usage 2 get
```

The `--debug` flag prints the raw bytes of every HID request and
response — useful when picking the interface manually or debugging
unexpected behaviour.

## Tests

```bash
cargo test
```

Verifies DPI encoding/decoding against the traffic-confirmed data points,
plus a full roundtrip across the entire value range.

## Acknowledgements

This project is built on top of the [`libatk-rs`](https://github.com/cyberphantom52/libatk-rs)
crate by [cyberphantom52](https://github.com/cyberphantom52), used here as
a regular dependency (see `Cargo.toml`) rather than vendored code. That
library provides the base structure of the ATK protocol (command format,
`EEPROMAddress`, HID report handling) — without it this project would
have started from zero. The DPI-specific encoding formula (see above)
was derived separately, by reverse-engineering real HID traffic.
Distributed under GPL-3.0 (see `LICENSE`), matching `libatk-rs`'s own
license.
