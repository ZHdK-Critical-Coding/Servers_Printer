// Finds printers and sends data to them: USB (libusb), device files (/dev/lp0) and CUPS queues
use rusb::UsbContext;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const PRINTER_CLASS: u8 = 0x07;
const CHUNK_SIZE: usize = 16 * 1024;
const USB_TIMEOUT: Duration = Duration::from_secs(60);

// How a printer is driven, which also decides which WebSocket port uses it
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    // ESC/POS receipt printer, protocol of printer-websockets (port 8080)
    Thermal,
    // ESC/P dot matrix printer, protocol of lp-websockets (port 8090)
    Line,
    // Any CUPS printer, the sketch is printed as an image on a page (port 8070)
    Image,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Thermal, Mode::Line, Mode::Image];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Thermal => "Thermal (ESC/POS)",
            Mode::Line => "Line printer (ESC/P)",
            Mode::Image => "CUPS image",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Transport {
    Usb { vendor: u16, product: u16, bus: u8, ports: Vec<u8> },
    File { path: String },
    Cups { queue: String },
    // --simulate: jobs are saved to a folder
    Simulated { folder: PathBuf },
}

#[derive(Clone, Debug)]
pub struct Device {
    // Stable id, used to remember the selection and mode
    pub key: String,
    pub name: String,
    pub detail: String,
    pub transport: Transport,
}

impl Device {
    // Raw ESC/P(OS) data can go to every device, the image mode needs CUPS (or the simulation)
    pub fn modes(&self) -> Vec<Mode> {
        match self.transport {
            Transport::Usb { .. } | Transport::File { .. } => vec![Mode::Thermal, Mode::Line],
            _ => Mode::ALL.to_vec(),
        }
    }

    pub fn default_mode(&self) -> Mode {
        match &self.transport {
            // Epson TM printers have the USB ids 04b8:0e.. and "TM-" in their name
            Transport::Usb { vendor, product, .. } => {
                if (*vendor == 0x04b8 && product >> 8 == 0x0e) || self.name.contains("TM-") {
                    Mode::Thermal
                } else {
                    Mode::Line
                }
            }
            Transport::File { .. } => Mode::Line,
            Transport::Cups { .. } => Mode::Image,
            Transport::Simulated { .. } => match self.key.as_str() {
                "sim:thermal" => Mode::Thermal,
                "sim:line" => Mode::Line,
                _ => Mode::Image,
            },
        }
    }
}

// What a job sends to the device
pub enum Job {
    // ESC/P or ESC/POS bytes, with a PNG of the printed dots for the simulation
    Raw { data: Vec<u8>, preview: Option<Vec<u8>> },
    // A PDF page for CUPS
    Pdf { data: Vec<u8>, media: String, copies: u32 },
    // Plain text for CUPS (test page)
    Text(String),
}

// Remembers the USB and CUPS names, reading them is slow (USB: the device has to be opened)
#[derive(Default)]
pub struct Discovery {
    usb_names: HashMap<String, String>,
    // Name and address per CUPS queue
    cups_info: HashMap<String, (String, String)>,
    simulate: Option<PathBuf>,
}

impl Discovery {
    pub fn new(simulate: Option<PathBuf>) -> Discovery {
        Discovery { simulate, ..Default::default() }
    }

    pub fn scan(&mut self) -> Vec<Device> {
        let mut devices = self.usb();
        devices.extend(device_files());
        devices.extend(cups_queues(&mut self.cups_info));
        if let Some(folder) = &self.simulate {
            for (key, name) in [
                ("sim:thermal", "Simulated thermal printer"),
                ("sim:line", "Simulated line printer"),
                ("sim:image", "Simulated CUPS printer"),
            ] {
                devices.push(Device {
                    key: key.into(),
                    name: name.into(),
                    detail: format!("Saves jobs to {}", home_relative(folder)),
                    transport: Transport::Simulated { folder: folder.clone() },
                });
            }
        }
        devices
    }

    fn usb(&mut self) -> Vec<Device> {
        let Ok(list) = rusb::devices() else { return vec![] };
        let mut printers: Vec<_> = list
            .iter()
            .filter(is_printer)
            .filter_map(|device| {
                let descriptor = device.device_descriptor().ok()?;
                let ports = device.port_numbers().unwrap_or_default();
                Some((device, descriptor, ports))
            })
            .collect();
        // Same order on every scan, so identical printers keep their number
        printers.sort_by_key(|(device, _, ports)| (device.bus_number(), ports.clone()));

        let mut devices: Vec<Device> = Vec::new();
        for (device, descriptor, ports) in printers {
            let (vendor, product) = (descriptor.vendor_id(), descriptor.product_id());
            let path = ports.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(".");
            // The key does not contain the USB port, so the printer is remembered in any port.
            // Identical printers are told apart by number: usb:04b8:0e02, usb:04b8:0e02#2, ...
            let base = format!("usb:{vendor:04x}:{product:04x}");
            let count = devices.iter().filter(|d| d.key == base || d.key.starts_with(&format!("{base}#"))).count();
            let key = if count == 0 { base } else { format!("{base}#{}", count + 1) };
            let name = self
                .usb_names
                .entry(format!("{key}@{}-{path}", device.bus_number()))
                .or_insert_with(|| usb_name(&device, &descriptor))
                .clone();
            devices.push(Device {
                key,
                name,
                detail: format!("USB {vendor:04x}:{product:04x}, bus {} port {path}", device.bus_number()),
                transport: Transport::Usb { vendor, product, bus: device.bus_number(), ports },
            });
        }
        devices
    }
}

// "~/Documents/..." instead of the full path with the user name
fn home_relative(path: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(PathBuf::from)) {
        Some(relative) => format!("~/{}", relative.display()),
        None => path.display().to_string(),
    }
}

fn is_printer<T: UsbContext>(device: &rusb::Device<T>) -> bool {
    let Ok(config) = device.active_config_descriptor().or_else(|_| device.config_descriptor(0)) else {
        return false;
    };
    config
        .interfaces()
        .any(|i| i.descriptors().any(|d| d.class_code() == PRINTER_CLASS))
}

fn usb_name<T: UsbContext>(device: &rusb::Device<T>, descriptor: &rusb::DeviceDescriptor) -> String {
    let fallback = format!("USB printer {:04x}:{:04x}", descriptor.vendor_id(), descriptor.product_id());
    let Ok(handle) = device.open() else { return fallback };
    let manufacturer = handle.read_manufacturer_string_ascii(descriptor).unwrap_or_default();
    let product = handle.read_product_string_ascii(descriptor).unwrap_or_default();
    let name = format!("{} {}", manufacturer.trim(), product.trim()).trim().to_string();
    if name.is_empty() { fallback } else { name }
}

// Printer device files of the Linux kernel driver: /dev/usb/lp0 ... (USB) and /dev/lp0 ... (parallel)
fn device_files() -> Vec<Device> {
    let mut devices = Vec::new();
    for dir in ["/dev/usb", "/dev"] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.len() > 2 && n.starts_with("lp") && n[2..].chars().all(|c| c.is_ascii_digit()))
            .collect();
        names.sort();
        for name in names {
            let path = format!("{dir}/{name}");
            devices.push(Device {
                key: format!("file:{path}"),
                name: format!("Device file {path}"),
                detail: "Direct line printing".into(),
                transport: Transport::File { path },
            });
        }
    }
    devices
}

// CUPS printer queues. The output of `lpstat` is translated on macOS, so only the bare
// queue names of `lpstat -e` and the attributes of `lpoptions` are read.
fn cups_queues(cache: &mut HashMap<String, (String, String)>) -> Vec<Device> {
    let Ok(output) = Command::new("lpstat").arg("-e").stderr(Stdio::null()).output() else {
        return vec![];
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|queue| !queue.is_empty())
        .map(|queue| {
            let (name, uri) = cache.entry(queue.to_string()).or_insert_with(|| cups_info(queue)).clone();
            Device {
                key: format!("cups:{queue}"),
                name,
                detail: format!("CUPS queue {queue}, {uri}"),
                transport: Transport::Cups { queue: queue.to_string() },
            }
        })
        .collect()
}

// Description and device address of a queue, from `lpoptions -p queue`:
// key=value pairs, values with spaces in single quotes
fn cups_info(queue: &str) -> (String, String) {
    let output = Command::new("lpoptions").args(["-p", queue]).stderr(Stdio::null()).output();
    let text = output.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let mut options = HashMap::new();
    let mut chars = text.chars().peekable();
    while chars.peek().is_some() {
        let key: String = chars.by_ref().skip_while(|c| c.is_whitespace()).take_while(|c| *c != '=').collect();
        let mut value = String::new();
        let mut quoted = false;
        while let Some(c) = chars.next() {
            match c {
                '\\' => value.extend(chars.next()),
                '\'' => quoted = !quoted,
                ' ' if !quoted => break,
                _ => value.push(c),
            }
        }
        options.insert(key, value);
    }
    let name = options.get("printer-info").filter(|n| !n.is_empty()).cloned().unwrap_or(queue.to_string());
    let uri = options.get("device-uri").cloned().unwrap_or_default();
    // Keep the address short: no query and no URL escapes like %20
    (name, percent_decode(uri.split('?').next().unwrap_or("")))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// Sends a job to the device. Blocks until the data is written (or handed to CUPS).
pub fn send(device: &Device, job: Job) -> Result<(), String> {
    match (&device.transport, job) {
        (Transport::Usb { vendor, product, bus, ports }, Job::Raw { data, .. }) => {
            write_usb(*vendor, *product, *bus, ports, &data)
        }
        (Transport::File { path }, Job::Raw { data, .. }) => write_file(path, &data),
        (Transport::Cups { queue }, Job::Raw { data, .. }) => lp(queue, &["-o", "raw"], &data),
        (Transport::Cups { queue }, Job::Pdf { data, media, copies }) => {
            let copies = copies.max(1).to_string();
            let media = format!("media={media}");
            lp(queue, &["-n", &copies, "-o", &media, "-t", "p5 sketch"], &data)
        }
        (Transport::Cups { queue }, Job::Text(text)) => lp(queue, &["-t", "Printer Server"], text.as_bytes()),
        (Transport::Simulated { folder }, job) => simulate(folder, &device.key, job),
        _ => Err("This printer does not support this kind of job".into()),
    }
}

fn write_usb(vendor: u16, product: u16, bus: u8, ports: &[u8], data: &[u8]) -> Result<(), String> {
    let list = rusb::devices().map_err(|e| e.to_string())?;
    let device = list
        .iter()
        .find(|d| {
            d.bus_number() == bus
                && d.port_numbers().ok().as_deref() == Some(ports)
                && d.device_descriptor().is_ok_and(|x| x.vendor_id() == vendor && x.product_id() == product)
        })
        .ok_or("Printer not found on USB")?;

    let config = device
        .active_config_descriptor()
        .or_else(|_| device.config_descriptor(0))
        .map_err(|e| e.to_string())?;
    let (number, setting, endpoint) = config
        .interfaces()
        .flat_map(|i| i.descriptors())
        .filter(|d| d.class_code() == PRINTER_CLASS)
        .find_map(|d| {
            let out = d.endpoint_descriptors().find(|e| {
                e.direction() == rusb::Direction::Out && e.transfer_type() == rusb::TransferType::Bulk
            })?;
            Some((d.interface_number(), d.setting_number(), out.address()))
        })
        .ok_or("Printer has no USB out endpoint")?;

    let handle = device.open().map_err(|e| format!("Could not open USB printer: {e}"))?;
    // Linux: the kernel printer driver (usblp) has to let go of the printer first
    let _ = handle.set_auto_detach_kernel_driver(true);
    handle.claim_interface(number).map_err(|e| {
        format!("USB printer is in use ({e}). Is a system print job running? Use its CUPS queue instead.")
    })?;
    // Only when needed: switching the setting resets the printer's USB endpoint, which can
    // drop data of the previous job that the printer has not printed yet
    if setting != 0 {
        let _ = handle.set_alternate_setting(number, setting);
    }

    let mut total = 0;
    let mut retried = false;
    let result = data.chunks(CHUNK_SIZE).try_for_each(|chunk| {
        let mut sent = 0;
        while sent < chunk.len() {
            match handle.write_bulk(endpoint, &chunk[sent..], USB_TIMEOUT) {
                Ok(n) => {
                    sent += n;
                    total += n;
                }
                // The endpoint is stuck from an interrupted job ("Pipe error" or, with Prolific
                // parallel adapters, "Input/Output Error"): reset it and try again. Clearing the
                // halt is not enough for these adapters, selecting the setting again resets them.
                // Only before any data went through, so nothing is sent twice.
                Err(rusb::Error::Pipe | rusb::Error::Io) if total == 0 && !retried => {
                    retried = true;
                    let _ = handle.clear_halt(endpoint);
                    let _ = handle.set_alternate_setting(number, setting);
                }
                Err(e) => return Err(format!("USB write failed: {e}{}", port_status(&handle, number))),
            }
        }
        Ok(())
    });
    let _ = handle.release_interface(number);
    result
}

// Reason from the printer class request GET_PORT_STATUS, e.g. " (printer is out of paper)"
fn port_status<T: UsbContext>(handle: &rusb::DeviceHandle<T>, interface: u8) -> String {
    let mut status = [0u8; 1];
    let request_type = rusb::request_type(rusb::Direction::In, rusb::RequestType::Class, rusb::Recipient::Interface);
    match handle.read_control(request_type, 1, 0, (interface as u16) << 8, &mut status, Duration::from_secs(1)) {
        Ok(1) if status[0] & 0x20 != 0 => " (printer is out of paper)".into(),
        Ok(1) if status[0] & 0x10 == 0 => " (printer is offline: press its online / select button)".into(),
        Ok(1) if status[0] & 0x08 == 0 => " (printer reports an error: check paper, cover and ribbon)".into(),
        _ => String::new(),
    }
}

fn write_file(path: &str, data: &[u8]) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new().write(true).open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("No permission for {path}. Run: sudo usermod -aG lp $USER (then log in again)")
        } else {
            format!("{path}: {e}")
        }
    })?;
    // The printer driver accepts only a limited amount at once
    for chunk in data.chunks(CHUNK_SIZE) {
        file.write_all(chunk).map_err(|e| format!("{path}: {e}"))?;
    }
    file.flush().map_err(|e| format!("{path}: {e}"))
}

fn lp(queue: &str, options: &[&str], data: &[u8]) -> Result<(), String> {
    let mut child = Command::new("lp")
        .arg("-d")
        .arg(queue)
        .args(options)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start lp: {e}"))?;
    child.stdin.take().unwrap().write_all(data).map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if error.is_empty() { format!("lp failed ({})", output.status) } else { error })
    }
}

fn simulate(folder: &std::path::Path, key: &str, job: Job) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let count = COUNT.fetch_add(1, Ordering::Relaxed);
    let base = folder.join(format!("{}-{millis}-{count}", key.trim_start_matches("sim:")));
    let write = |ext: &str, data: &[u8]| std::fs::write(base.with_extension(ext), data).map_err(|e| e.to_string());
    match job {
        Job::Raw { data, preview } => {
            write("bin", &data)?;
            if let Some(png) = preview {
                write("png", &png)?;
            }
            Ok(())
        }
        Job::Pdf { data, .. } => write("pdf", &data),
        Job::Text(text) => write("txt", text.as_bytes()),
    }
}
