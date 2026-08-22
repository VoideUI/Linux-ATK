use libatk_rs::prelude::*;
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use hidapi::HidApi;

/// Known Vendor IDs for ATK/VXE devices.
const ATK_VENDOR_IDS: [u16; 2] = [0x373b, 0x3554];

/// Candidate (usage_page, usage) pairs for the vendor-specific interface
/// the ATK protocol lives on. Order matters: ff02/2 is confirmed working
/// on a real device (VXE NordicMouse 1K Dongle) and comes first; the rest
/// are other usage pages of the same interface (iface=1) seen in `list`
/// output on the same device, as fallbacks for other models/firmwares.
const INTERFACE_CANDIDATES: [(u16, u16); 5] = [
    (0xff02, 0x0002),
    (0xff03, 0x0000),
    (0xff04, 0x0002),
    (0xff05, 0x0000),
    (0xff00, 0x0001),
];

/// DPI step, confirmed by traffic and stated by the manufacturer (100-30000, step 50).
const DPI_STEP: u32 = 50;

/// EEPROM addresses for the 4 DPI profile pairs, in order DPI1/2, DPI3/4, DPI5/6, DPI7/8.
const DPI_PAIR_ADDRESSES: [EEPROMAddress; 4] = [
    EEPROMAddress::DpiPair1,
    EEPROMAddress::DpiPair3,
    EEPROMAddress::DpiPair5,
    EEPROMAddress::DpiPair7,
];

struct EepromCommand;
impl CommandDescriptor for EepromCommand {}

#[derive(Parser)]
#[command(
    name = "atk-dpi",
    version,
    about = "Manage DPI on ATK/VXE mice over HID"
)]
struct Cli {
    /// Explicitly specify the device Vendor ID in hex (e.g. 373b). Usually
    /// not needed — detected automatically.
    #[arg(long, value_parser = parse_hex_u16)]
    vid: Option<u16>,

    /// Explicitly specify the device Product ID in hex (e.g. f58a). Usually
    /// not needed — detected automatically.
    #[arg(long, value_parser = parse_hex_u16)]
    pid: Option<u16>,

    /// HID interface usage page. Usually not needed — the utility tries
    /// known candidates itself and finds a working one. Set explicitly
    /// only if auto-detection failed (see `atk-dpi list` for actual values).
    #[arg(long, value_parser = parse_hex_u16)]
    usage_page: Option<u16>,

    /// HID interface usage (see usage-page above).
    #[arg(long, value_parser = parse_hex_u16)]
    usage: Option<u16>,

    /// Print the raw bytes of every HID request and response (for debugging).
    #[arg(long)]
    debug: bool,

    #[command(subcommand)]
    command: Command_,
}

#[derive(Subcommand)]
enum Command_ {
    /// List all connected ATK/VXE HID devices.
    List,

    /// Read all 8 DPI profiles from the mouse.
    #[command(alias = "get-dpi")]
    Get,

    /// Set DPI for one of the 8 profiles. Example: atk-dpi set 5 3500
    #[command(alias = "set-dpi")]
    Set {
        /// Profile number, 1-8 (corresponds to DPI1..DPI8 in ATK HUB).
        slot: u8,

        /// DPI value, a multiple of 50, in the range 100-30000.
        value: u32,
    },

    /// Switch the active DPI profile (does not change the value, only
    /// selects one of the already configured 8 profiles). Example:
    /// atk-dpi select 5
    #[command(alias = "select-dpi")]
    Select {
        /// Profile number, 1-8 (corresponds to DPI1..DPI8 in ATK HUB).
        slot: u8,
    },
}

fn parse_hex_u16(s: &str) -> Result<u16, String> {
    u16::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|e| e.to_string())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let api = HidApi::new().context("failed to initialize HID API")?;

    match cli.command {
        Command_::List => list_devices(&api, cli.vid),
        Command_::Get => {
            let device = connect(&api, &cli)?;
            get_dpi(&device, cli.debug)
        }
        Command_::Set { slot, value } => {
            let device = connect(&api, &cli)?;
            set_dpi(&device, slot, value, cli.debug)
        }
        Command_::Select { slot } => {
            let device = connect(&api, &cli)?;
            select_dpi(&device, slot, cli.debug)
        }
    }
}

/// Finds the device and opens a working HID interface: if usage_page/usage
/// are given explicitly, uses them; otherwise tries known candidates and
/// probes each with a read until one responds.
fn connect(api: &HidApi, cli: &Cli) -> Result<Device> {
    let (vid, pid) = resolve_ids(api, cli.vid, cli.pid)?;

    if let (Some(usage_page), Some(usage)) = (cli.usage_page, cli.usage) {
        return open_device(vid, pid, usage_page, usage);
    }

    autodetect_interface(vid, pid, cli.debug)
}

fn list_devices(api: &HidApi, vid_filter: Option<u16>) -> Result<()> {
    let mut found = false;
    for dev in api.device_list() {
        let matches_vendor = match vid_filter {
            Some(vid) => dev.vendor_id() == vid,
            None => ATK_VENDOR_IDS.contains(&dev.vendor_id()),
        };
        if !matches_vendor {
            continue;
        }
        found = true;
        println!(
            "VID={:04x} PID={:04x}  usage_page={:#06x} usage={:#06x}  iface={}  {} — {}",
            dev.vendor_id(),
            dev.product_id(),
            dev.usage_page(),
            dev.usage(),
            dev.interface_number(),
            dev.manufacturer_string().unwrap_or("?"),
            dev.product_string().unwrap_or("?"),
        );
    }
    if !found {
        println!(
            "No ATK/VXE devices found. Check that:\n\
             1) the mouse is connected via cable or 2.4G dongle (Bluetooth often does not allow raw HID access);\n\
             2) on Linux — a udev rule is installed for root-free access\n\
                (see https://gist.github.com/Speyll/b7803161b1ee43f258d484fe9e92c4b4);\n\
             3) the device VID is not in the list {:04x?} — find it via\n\
                `lsusb` (Linux) or Device Manager (Windows) and pass --vid explicitly.",
            ATK_VENDOR_IDS
        );
    }
    Ok(())
}

fn resolve_ids(api: &HidApi, vid: Option<u16>, pid: Option<u16>) -> Result<(u16, u16)> {
    if let (Some(v), Some(p)) = (vid, pid) {
        return Ok((v, p));
    }
    for dev in api.device_list() {
        let matches_vendor = match vid {
            Some(v) => dev.vendor_id() == v,
            None => ATK_VENDOR_IDS.contains(&dev.vendor_id()),
        };
        if matches_vendor {
            return Ok((dev.vendor_id(), dev.product_id()));
        }
    }
    bail!(
        "Could not automatically detect the device. \
         Run `atk-dpi list`, find your mouse and pass --vid/--pid explicitly."
    )
}

fn open_device(vid: u16, pid: u16, usage_page: u16, usage: u16) -> Result<Device> {
    Device::new(vid, pid, usage_page, usage).map_err(|e| {
        anyhow::anyhow!(
            "failed to open device {vid:04x}:{pid:04x} \
             (usage_page={usage_page:#06x}, usage={usage:#06x}): {e}\n\
             Try `atk-dpi list` to see the actual usage_page/usage values \
             for all interfaces of this device."
        )
    })
}

/// Tries known (usage_page, usage) candidates for a device with the given
/// VID/PID, probing each with a harmless read (ReportRate address, 10
/// bytes) — the first one that replies with status=0 is considered the
/// working ATK protocol interface.
fn autodetect_interface(vid: u16, pid: u16, debug: bool) -> Result<Device> {
    for &(usage_page, usage) in INTERFACE_CANDIDATES.iter() {
        let Ok(device) = Device::new(vid, pid, usage_page, usage) else {
            continue;
        };
        if debug {
            eprintln!("[debug] trying usage_page={usage_page:#06x} usage={usage:#06x}...");
        }
        match read_eeprom(&device, EEPROMAddress::ReportRate, 10, false) {
            Ok(data) if !data.is_empty() => {
                if debug {
                    eprintln!(
                        "[debug] usage_page={usage_page:#06x} usage={usage:#06x} worked"
                    );
                }
                return Ok(device);
            }
            _ => continue,
        }
    }
    bail!(
        "Could not find a working HID interface among the known candidates \
         for device {vid:04x}:{pid:04x}. Run `atk-dpi list`, find the \
         vendor-specific interface (usage_page is usually 0xffXX), and pass \
         it explicitly via --usage-page/--usage."
    )
}

/// Encodes a DPI value into a 4-byte block for a single profile.
///
/// The formula has been confirmed against real HID traffic at 8
/// independent points, including crossing the idx_hi=0/1 and idx_hi=1/2
/// boundaries (DPI 1200, 1300, 1400, 1500, 12700, 13000, 20000, 30000 —
/// all matched in value and checksum).
fn encode_dpi_block(dpi: u32) -> Result<[u8; 4]> {
    if dpi < DPI_STEP || dpi % DPI_STEP != 0 {
        bail!("DPI must be a multiple of {DPI_STEP} and at least {DPI_STEP}, got {dpi}");
    }
    let idx = dpi / DPI_STEP - 1;
    let idx_hi_check = (idx >> 8) & 0xff;
    if idx_hi_check > 3 {
        // idx_hi*0x44 must not overflow u8 (0xff/0x44 = 3 max).
        bail!("DPI is too large to encode (idx={idx})");
    }
    let idx_lo = (idx & 0xff) as u8;
    let idx_hi = ((idx >> 8) & 0xff) as u8;
    let byte2 = idx_hi.wrapping_mul(0x44);
    let checksum = 0x55u8
        .wrapping_sub(idx_lo)
        .wrapping_sub(idx_lo)
        .wrapping_sub(byte2);
    Ok([idx_lo, idx_lo, byte2, checksum])
}

/// Decodes a 4-byte block back into a DPI value.
fn decode_dpi_block(block: &[u8]) -> u32 {
    let idx_lo = block[0] as u32;
    // byte2 = idx_hi * 0x44 (68) — inverted via division; on corrupted data
    // (byte2 not a multiple of 0x44) the result will be approximate, but
    // that's better than panicking on read.
    let idx_hi = (block[2] as u32) / 0x44;
    let idx = (idx_hi << 8) | idx_lo;
    (idx + 1) * DPI_STEP
}

fn read_eeprom(
    device: &Device,
    address: EEPROMAddress,
    expected_len: usize,
    debug: bool,
) -> Result<Vec<u8>> {
    let mut cmd = Command::<EepromCommand>::default();
    cmd.set_eeprom_address(address);
    cmd.set_id(CommandId::GetEEPROM);
    // IMPORTANT: GetEEPROM requires explicitly stating how many bytes are
    // being requested — without it the device replies with status=1
    // (error) and data_len=0. Confirmed empirically on a real device (VXE
    // NordicMouse 1K Dongle): a request with data_len=0 produced an empty
    // reply; in previously captured traffic from the real ATK HUB,
    // GetEEPROM requests also always specified a concrete length (e.g. 8
    // for Key addresses).
    cmd.set_data_len(expected_len)?;

    if debug {
        eprintln!("[debug] request:  {:02x?}", cmd.as_bytes());
    }

    let response = cmd
        .execute(device)
        .map_err(|e| anyhow::anyhow!("error reading EEPROM {address:?}: {e}"))?;

    if debug {
        eprintln!("[debug] response: {:02x?}", response.as_bytes());
        eprintln!(
            "[debug] parsed: cmd_id={:?} status={} addr={:?} data_len={} data={:02x?}",
            response.id(),
            response.status(),
            response.eeprom_address(),
            response.data_len(),
            response.data()
        );
    }

    if response.status() != 0 {
        bail!(
            "device returned an error while reading EEPROM {address:?}: status={}",
            response.status()
        );
    }

    Ok(response.data()[..response.data_len()].to_vec())
}

fn write_eeprom(device: &Device, address: EEPROMAddress, data: &[u8], debug: bool) -> Result<()> {
    let mut cmd = Command::<EepromCommand>::default();
    cmd.set_id(CommandId::SetEEPROM);
    cmd.set_eeprom_address(address);
    cmd.set_data_len(data.len())?;
    cmd.set_data(data, 0)?;

    if debug {
        eprintln!("[debug] request:  {:02x?}", cmd.as_bytes());
    }

    let response = cmd
        .execute(device)
        .map_err(|e| anyhow::anyhow!("error writing EEPROM {address:?}: {e}"))?;

    if debug {
        eprintln!("[debug] response: {:02x?}", response.as_bytes());
    }

    if response.status() != 0 {
        bail!(
            "device returned an error while writing EEPROM {address:?}: status={}",
            response.status()
        );
    }
    Ok(())
}

/// Reads all 8 DPI profiles from the mouse.
fn get_dpi(device: &Device, debug: bool) -> Result<()> {
    for (pair_idx, &addr) in DPI_PAIR_ADDRESSES.iter().enumerate() {
        let data = read_eeprom(device, addr, 8, debug)?;
        if data.len() < 8 {
            println!(
                "Profiles {}/{}: received fewer than 8 bytes of data ({}), skipping",
                pair_idx * 2 + 1,
                pair_idx * 2 + 2,
                data.len()
            );
            continue;
        }
        let dpi_a = decode_dpi_block(&data[0..4]);
        let dpi_b = decode_dpi_block(&data[4..8]);
        println!("DPI{}: {} dpi", pair_idx * 2 + 1, dpi_a);
        println!("DPI{}: {} dpi", pair_idx * 2 + 2, dpi_b);
    }
    Ok(())
}

/// Sets DPI for a single profile (1-8), leaving the other 7 profiles
/// untouched — first reads the whole pair containing the target slot,
/// changes only one 4-byte block, and sends the pair back in full.
fn set_dpi(device: &Device, slot: u8, value: u32, debug: bool) -> Result<()> {
    if !(1..=8).contains(&slot) {
        bail!("Profile number must be between 1 and 8, got {slot}");
    }

    let pair_index = (slot - 1) / 2; // 0..=3, which of the 4 pairs
    let is_second_in_pair = (slot - 1) % 2 == 1; // first or second profile of the pair
    let addr = DPI_PAIR_ADDRESSES[pair_index as usize];

    let mut data = read_eeprom(device, addr, 8, debug)?;
    if data.len() < 8 {
        bail!(
            "Expected 8 bytes of data when reading {addr:?} before writing, got {}. \
             Aborting to avoid losing the neighboring profile.",
            data.len()
        );
    }

    let new_block = encode_dpi_block(value)?;
    let offset = if is_second_in_pair { 4 } else { 0 };
    data[offset..offset + 4].copy_from_slice(&new_block);

    write_eeprom(device, addr, &data, debug)?;

    println!("DPI{slot} set to {value} dpi.");
    Ok(())
}

/// Switches the active DPI profile. Format: EEPROMAddress::ReportRate
/// (address 0x0000) holds a 10-byte block: [ReportRate, ReportRateCrc,
/// MaxDpi, MaxDpiCrc, CurrentDpi, CurrentDpiCrc, ...]. The byte at offset
/// 4 (CurrentDpi) is the 0-based index of the active profile (slot-1),
/// the byte at offset 5 is its checksum (0x55 - value). The formula was
/// confirmed by real traffic early in the protocol work (a packet with
/// index 7 corresponded to the active profile being DPI8).
///
/// Reads the whole 10-byte block, changes only bytes 4 and 5, and sends
/// the rest (ReportRate, MaxDpi and their CRCs) back unchanged — just like
/// `set_dpi`, so as not to disturb settings unrelated to DPI.
fn select_dpi(device: &Device, slot: u8, debug: bool) -> Result<()> {
    if !(1..=8).contains(&slot) {
        bail!("Profile number must be between 1 and 8, got {slot}");
    }

    let mut data = read_eeprom(device, EEPROMAddress::ReportRate, 10, debug)?;
    if data.len() < 6 {
        bail!(
            "Expected at least 6 bytes of data when reading the ReportRate/CurrentDpi \
             block before writing, got {}. Aborting to avoid losing other settings.",
            data.len()
        );
    }

    let idx = slot - 1;
    data[4] = idx;
    data[5] = 0x55u8.wrapping_sub(idx);

    write_eeprom(device, EEPROMAddress::ReportRate, &data, debug)?;

    println!("Active profile switched to DPI{slot}.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_confirmed_points() {
        // Points confirmed via real HID traffic (see comment at the top of the file).
        assert_eq!(encode_dpi_block(1200).unwrap(), [0x17, 0x17, 0x00, 0x27]);
        assert_eq!(encode_dpi_block(1300).unwrap(), [0x19, 0x19, 0x00, 0x23]);
        assert_eq!(encode_dpi_block(1400).unwrap(), [0x1b, 0x1b, 0x00, 0x1f]);
        assert_eq!(encode_dpi_block(1500).unwrap(), [0x1d, 0x1d, 0x00, 0x1b]);
        // High-range points (idx_hi != 0), confirming the byte2 formula.
        assert_eq!(encode_dpi_block(12700).unwrap(), [0xfd, 0xfd, 0x00, 0x5b]);
        assert_eq!(encode_dpi_block(13000).unwrap(), [0x03, 0x03, 0x44, 0x0b]);
        assert_eq!(encode_dpi_block(20000).unwrap(), [0x8f, 0x8f, 0x44, 0xf3]);
        assert_eq!(encode_dpi_block(30000).unwrap(), [0x57, 0x57, 0x88, 0x1f]);
    }

    #[test]
    fn test_decode_confirmed_points() {
        assert_eq!(decode_dpi_block(&[0x17, 0x17, 0x00, 0x27]), 1200);
        assert_eq!(decode_dpi_block(&[0x19, 0x19, 0x00, 0x23]), 1300);
        assert_eq!(decode_dpi_block(&[0x1b, 0x1b, 0x00, 0x1f]), 1400);
        assert_eq!(decode_dpi_block(&[0x1d, 0x1d, 0x00, 0x1b]), 1500);
        assert_eq!(decode_dpi_block(&[0xfd, 0xfd, 0x00, 0x5b]), 12700);
        assert_eq!(decode_dpi_block(&[0x03, 0x03, 0x44, 0x0b]), 13000);
        assert_eq!(decode_dpi_block(&[0x8f, 0x8f, 0x44, 0xf3]), 20000);
        assert_eq!(decode_dpi_block(&[0x57, 0x57, 0x88, 0x1f]), 30000);
    }

    #[test]
    fn test_roundtrip() {
        for dpi in (100..=30000u32).step_by(50) {
            let block = encode_dpi_block(dpi).unwrap();
            assert_eq!(decode_dpi_block(&block), dpi, "roundtrip failed for {dpi}");
        }
    }

    #[test]
    fn test_rejects_non_multiple_of_50() {
        assert!(encode_dpi_block(1234).is_err());
    }
}
