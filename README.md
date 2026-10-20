# Rust: Printer Server for macOS

A small macOS app that makes printers connected to a Mac available to p5
sketches over WebSocket. It speaks exactly the same protocols as the
Raspberry Pi servers `printer-websockets` (thermal printer) and
`lp-websockets` (line printer) from
[Servers_RaspberriPi](https://github.com/ZHdK-Critical-Coding/Servers_RaspberriPi),
so the sketches
[P5_Output_Printers_ThermalPrinter_Lines](https://github.com/ZHdK-Critical-Coding/P5_Output_Printers_ThermalPrinter_Lines),
[P5_Output_Printers_ThermalPrinter_Bitmap](https://github.com/ZHdK-Critical-Coding/P5_Output_Printers_ThermalPrinter_Bitmap) and
[P5_Output_Printers_LinePrinter](https://github.com/ZHdK-Critical-Coding/P5_Output_Printers_LinePrinter)
work without changes: only `printerConnection` has to point to the Mac.
A third, new protocol prints the sketch as an image on any CUPS printer
(office printers, inkjets, everything in *System Settings → Printers*).

## Installation

Download `Printer-Server-<version>.dmg` from
[Releases](https://github.com/ZHdK-Critical-Coding/Servers_Printer/releases),
open it and drag **Printer Server** into *Applications*. The app is signed
and notarized and runs on Apple Silicon and Intel Macs with macOS 11 or
newer.

### Building it Yourself

Requirements: [Rust](https://rustup.rs/), macOS 11 or newer. libusb is built into the app, nothing else has to be installed.

```sh
./scripts/bundle-macos.sh              # dist/Printer Server.app for this Mac
./scripts/bundle-macos.sh --universal  # Apple Silicon and Intel
```

For `--universal`, add the targets once:
`rustup target add aarch64-apple-darwin x86_64-apple-darwin`.

### Signed and Notarized Build

To pass the app on to other Macs without security warnings, sign and
notarize it with a Developer ID:

```sh
./scripts/notarize-macos.sh               # Apple Silicon and Intel
./scripts/notarize-macos.sh --native      # only this Mac's architecture
./scripts/notarize-macos.sh --sign-only   # sign and check, send nothing to Apple
```

Results in `dist/`: the stapled app, a `.zip` and a `.dmg`. Needed once:
a *Developer ID Application* certificate in the keychain and notary
credentials stored with an
[app-specific password](https://appleid.apple.com):

```sh
xcrun notarytool store-credentials printer-server \
  --apple-id you@example.com --team-id ABCDE12345 --password xxxx-xxxx-xxxx-xxxx
```

Other names: `NOTARY_PROFILE=<name>`, other certificate:
`IDENTITY="Developer ID Application: ..."`.

Dependencies (`Cargo.toml`): `eframe` (GUI), `tokio` +
`tokio-tungstenite` (WebSocket), `rusb` (USB, with libusb built in),
`image` (PNG/JPEG), `flate2` (PDF compression).

## How to Run

1. Start `dist/Printer Server.app` (or `cargo run` while developing). On
   first start, allow incoming connections when macOS asks.
2. The window lists all printers found: USB printers, printer device files
   and CUPS queues. Plugging a printer in or out updates the list.
3. Choose the **mode** of each printer in its drop-down and **check** the
   printer that should print. There is one checked printer per mode, so a
   thermal printer, a line printer and a CUPS printer can be used at the
   same time. The selection is remembered.
4. Put the address shown at the top into the sketch, e.g.
   `const printerConnection = 'ws://10.21.3.49:8080'`.

**Test** prints the server address on a printer, like the Pi servers do on
start.

| Mode | Port | Protocol | Sketch |
| --- | --- | --- | --- |
| Thermal (ESC/POS) | 8080 | `printer-websockets` | ThermalPrinter_Lines, ThermalPrinter_Bitmap |
| Line printer (ESC/P) | 8090 | `lp-websockets` | LinePrinter |
| CUPS image | 8070 | new, see below | `examples/P5_Output_Printers_CUPS` |

### Without a Printer: Simulation

```sh
cargo run -- --simulate
```

adds three simulated printers that save every job to
`~/Documents/printer-simulation/`: the raw printer bytes (`.bin`) and a
`.png` of the dots the printer would print, or the `.pdf` for CUPS.

### Port Already in Use

If another program uses a port, the server takes the next free one
(8080 → 8081 → 8082 …, up to 20 tries). The address is then shown in
orange and noted in the log: put that address into the sketch. To choose
other start ports:

```sh
"dist/Printer Server.app/Contents/MacOS/printer-server" --thermal-port 9080 --line-port 9090 --cups-port 9070
```

On some Macs, Safari's WebDriver (`safaridriver`) occupies 8080 while it
runs.

## Coding Help

### Connections

| Connection | Found by | Used for |
| --- | --- | --- |
| USB | every USB device with the printer interface class | Thermal or Line mode, data goes directly to the printer (libusb) |
| Device file | `/dev/lp0`, `/dev/usb/lp0` … (Linux) | Thermal or Line mode, direct line printing |
| CUPS queue | `lpstat -e` | CUPS image mode, or Thermal / Line mode sent raw (`lp -o raw`) |

Epson TM printers (USB ids `04b8:0e..` or "TM-" in the name) start in
Thermal mode, other USB printers in Line mode, CUPS queues in CUPS image
mode.

On macOS the app talks to USB printers directly. This fails while a system
print job is running on the same printer: use its CUPS queue with Thermal
or Line mode instead.

### Protocol: Thermal Printer (port 8080)

Same as `printer-websockets`. Every request gets exactly one answer; the
server never sends anything else.

| Request | Answer |
| --- | --- |
| Text `{"text": "...", "w": 0, "h": 0, "style": "bu", "cut": false}` | `ok` |
| Text `cut` | `ok` |
| Binary PNG/JPEG: scaled to 256 dots, squeezed 1.6× vertically, threshold 128, centered | `ok` |
| Anything while the printer prints | `busy` |
| Broken JSON, no printer checked, printer error | `error: …` |

`w`, `h`: 0 = normal, 1 = double … 7 = eight times. `style`: `b` bold,
`u` underline.

### Protocol: Line Printer (port 8090)

Same as `lp-websockets`.

| Request | Answer |
| --- | --- |
| Text `{"text": "...", "w": 0, "h": 0, "style": "bu"}` | `ok` |
| Text `{"direction": "up" \| "down", "steps": 36}`, one step = 1/216 inch | `ok` |
| Binary PNG/JPEG: Floyd-Steinberg, 60 dpi, at most 480 dots (8 inches) | `printed_image` |
| Image could not be read or printed | `conversion_error` |
| Anything while the printer prints | `busy` |

### Protocol: CUPS Image (port 8070, new)

The sketch sends its canvas as PNG; the server places it on a PDF page and
sends it to the checked CUPS printer.

| Request | Answer |
| --- | --- |
| Binary PNG/JPEG | `ok` when the job is handed to CUPS |
| Text JSON with page options, valid for the following images of this connection | `ok` |
| Anything while the last job is handed over | `busy` |
| Errors | `error: …` |

Page options (all optional, missing ones keep their value):

```js
{
  media: 'A4',          // 'A3', 'A4', 'A5', 'A6', 'Letter', 'Legal'
  orientation: 'auto',  // 'auto' (follows the image), 'portrait', 'landscape'
  fit: true,            // true: fill the page inside the margin. false: size from dpi
  dpi: 96,              // only with fit: false. 96 = size on screen
  margin: 10,           // white border in mm
  copies: 1,
  dither: false,        // true: black and white dots, like the line printer
}
```

Use `pixelDensity(2)` or `3` in the sketch for a sharper print: the PNG
from `canvas.toBlob()` has the pixel density's resolution.

### Differences to the Raspberry Pi Servers

- Text is encoded as code page 437 (Umlauts work), like Electron_P5_Printers.
- Line printer images are scaled to at most 8 inches and corrected for the
  60 × 72 dpi dot shape, so retina canvases (pixelDensity 2) print at the
  right size. No ImageMagick / netpbm needed.
- Broken requests are answered with `error: …` instead of printing a help
  text on the paper.

### Project Structure

```
Servers_Printer/
├── src/
│   ├── main.rs       # Window (printer list, addresses, log), device scan
│   ├── server.rs     # WebSocket servers and the three protocols
│   ├── devices.rs    # Finding printers, USB / device file / CUPS / simulation
│   ├── state.rs      # Shared state, saved selection
│   ├── thermal.rs    # ESC/POS commands
│   ├── line.rs       # ESC/P commands
│   ├── bitmap.rs     # Image → black and white dots (threshold, dithering)
│   ├── pdf.rs        # Image → PDF page for CUPS
│   └── cp437.rs      # Text encoding
├── examples/
│   └── P5_Output_Printers_CUPS/   # Sketch for the CUPS image protocol
└── scripts/
    ├── bundle-macos.sh            # Builds the .app
    └── notarize-macos.sh          # Signs and notarizes app, zip and dmg
```

Image settings (thermal density and width, line printer resolution,
thresholds) are constants at the top of `thermal.rs` and `line.rs`.

The selection and modes are saved in
`~/Library/Application Support/printer-server/settings.json`. USB printers
are remembered by their USB ids (`usb:04b8:0e02`), so they stay selected in
any USB port; identical printers are numbered (`usb:04b8:0e02#2`). A
damaged settings file is kept as `settings.json.damaged`.
