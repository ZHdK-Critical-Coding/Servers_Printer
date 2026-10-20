// Printer Server: makes printers connected to a Mac available to p5 sketches over WebSocket,
// with the same protocols as the Raspberry Pi servers (printer-websockets, lp-websockets)
mod bitmap;
mod cp437;
mod devices;
mod line;
mod pdf;
mod server;
mod state;
mod thermal;

use devices::{Device, Discovery, Mode};
use eframe::egui;
use state::{Shared, State};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SCAN_INTERVAL: Duration = Duration::from_secs(3);
const NAME_WIDTH: f32 = 340.0;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    // --simulate adds printers that save every job to ~/Documents/printer-simulation
    let simulate = args
        .iter()
        .any(|a| a == "--simulate")
        .then(|| dirs::document_dir().unwrap_or_else(std::env::temp_dir).join("printer-simulation"));
    // --thermal-port, --line-port, --cups-port change the default ports 8080, 8090, 8070
    let mut ports = HashMap::new();
    for (flag, mode) in [("--thermal-port", Mode::Thermal), ("--line-port", Mode::Line), ("--cups-port", Mode::Image)] {
        if let Some(port) = args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)?.parse().ok()) {
            ports.insert(mode, port);
        }
    }
    server::set_ports(ports);

    let state: Shared = Arc::new(Mutex::new(State::load()));

    let runtime = tokio::runtime::Runtime::new().expect("Could not start the async runtime");
    for mode in Mode::ALL {
        runtime.spawn(server::serve(state.clone(), mode));
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Printer Server")
            .with_inner_size([820.0, 600.0])
            .with_min_inner_size([560.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Printer Server",
        options,
        Box::new(move |cc| {
            start_scanning(state.clone(), cc.egui_ctx.clone(), simulate);
            Ok(Box::new(App { state, _runtime: runtime }))
        }),
    )
}

// Looks for printers every few seconds, so plugging one in or out shows up in the list
fn start_scanning(state: Shared, ctx: egui::Context, simulate: Option<std::path::PathBuf>) {
    std::thread::spawn(move || {
        let mut discovery = Discovery::new(simulate);
        loop {
            let devices = discovery.scan();
            {
                let mut s = state.lock().unwrap();
                let old: Vec<String> = s.devices.iter().map(|d| d.key.clone()).collect();
                for device in &devices {
                    if !old.contains(&device.key) {
                        s.log(format!("Found {}", device.name));
                    }
                }
                for device in &s.devices.clone() {
                    if !devices.iter().any(|d| d.key == device.key) {
                        s.log(format!("Removed {}", device.name));
                    }
                }
                s.devices = devices;
            }
            ctx.request_repaint();
            std::thread::sleep(SCAN_INTERVAL);
        }
    });
}

struct App {
    state: Shared,
    // Runs the WebSocket servers as long as the window is open
    _runtime: tokio::runtime::Runtime,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Busy states and the log change without user input
        ui.ctx().request_repaint_after(Duration::from_millis(500));
        let mut s = self.state.lock().unwrap();

        egui::Panel::top("servers").show(ui, |ui| {
            ui.add_space(8.0);
            ui.heading("Printer Server");
            ui.label("Sketches connect to these addresses. Each one prints on the checked printer of its kind.");
            ui.add_space(4.0);
            egui::Grid::new("server-grid").num_columns(4).spacing([16.0, 4.0]).show(ui, |ui| {
                for mode in Mode::ALL {
                    ui.strong(mode.label());
                    if server::port(mode) == server::preferred_port(mode) {
                        ui.monospace(server::address(mode));
                    } else {
                        // Moved to the next free port: the sketch needs this address
                        let text = egui::RichText::new(server::address(mode)).monospace();
                        ui.colored_label(ui.visuals().warn_fg_color, text).on_hover_text(format!(
                            "Port {} is used by another program",
                            server::preferred_port(mode)
                        ));
                    }
                    if let Some(error) = s.port_errors.get(&mode) {
                        ui.colored_label(ui.visuals().error_fg_color, format!("Port not available: {error}"))
                            .on_hover_text("Quit the programs using these ports, or start with e.g. --thermal-port 9080");
                        ui.label("");
                        ui.end_row();
                        continue;
                    }
                    match s.active_device(mode) {
                        Some(device) => ui.label(format!("→ {}", device.name)),
                        None => ui.weak("→ no printer checked"),
                    };
                    let clients = s.clients.get(&mode).copied().unwrap_or(0);
                    ui.weak(match clients {
                        1 => "1 sketch".to_string(),
                        n => format!("{n} sketches"),
                    });
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
        });

        egui::Panel::bottom("log").resizable(true).default_size(160.0).show(ui, |ui| {
            ui.add_space(4.0);
            ui.strong("Log");
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
                for line in &s.log {
                    ui.monospace(line);
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.strong("Printers");
            ui.add_space(4.0);
            if s.devices.is_empty() {
                ui.weak("No printers found. Connect a USB printer or add one in System Settings → Printers.");
                return;
            }
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                egui::Grid::new("printer-grid").num_columns(4).spacing([12.0, 8.0]).striped(true).show(ui, |ui| {
                    for device in s.devices.clone() {
                        printer_row(ui, &mut s, &device, &self.state);
                        ui.end_row();
                    }
                });
            });
        });
    }
}

fn printer_row(ui: &mut egui::Ui, s: &mut State, device: &Device, shared: &Shared) {
    let mut active = s.is_active(device);
    if ui.checkbox(&mut active, "").changed() {
        s.set_active(device, active);
    }

    ui.vertical(|ui| {
        ui.set_width(NAME_WIDTH);
        ui.add(egui::Label::new(&device.name).truncate());
        ui.add(egui::Label::new(egui::RichText::new(&device.detail).small().weak()).truncate());
    });

    let mut mode = s.mode(device);
    egui::ComboBox::from_id_salt(&device.key).selected_text(mode.label()).width(170.0).show_ui(ui, |ui| {
        for option in device.modes() {
            ui.selectable_value(&mut mode, option, option.label());
        }
    });
    if mode != s.mode(device) {
        s.set_mode(device, mode);
    }

    ui.horizontal(|ui| {
        let busy = s.busy.contains(&device.key);
        if ui
            .add_enabled(!busy, egui::Button::new("Test"))
            .on_hover_text("Prints the server address on this printer")
            .clicked()
        {
            test_print(shared.clone(), s, device.clone(), mode);
        }
        if busy {
            ui.spinner();
        }
    });
}

fn test_print(shared: Shared, s: &mut State, device: Device, mode: Mode) {
    if !s.busy.insert(device.key.clone()) {
        return;
    }
    std::thread::spawn(move || {
        let result = devices::send(&device, server::test_job(&device, mode));
        let mut s = shared.lock().unwrap();
        s.busy.remove(&device.key);
        match result {
            Ok(()) => s.log(format!("Test page → {}", device.name)),
            Err(e) => s.log(format!("Error on {}: {e}", device.name)),
        }
    });
}
