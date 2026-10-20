// WebSocket servers. Ports and protocols are the same as on the Raspberry Pi:
//   8080  printer-websockets (thermal printer): JSON text, the string "cut", PNG as binary
//   8090  lp-websockets (line printer): JSON text or paper feed, PNG as binary
//   8070  new: PNG as binary is printed as an image on a CUPS printer, JSON sets page options
use crate::devices::{self, Device, Job, Mode};
use crate::pdf::{self, PageOptions};
use crate::state::Shared;
use crate::{bitmap, line, thermal};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

const THERMAL_FORMAT: &str = "Format: {text: string, w: int, h: int, style: string (ex. 'bu'), cut: bool}";
const LINE_FORMAT: &str =
    "Format: {text: string, w: int, h: int, style: string (ex. 'bu')} or {direction: 'up' | 'down', steps: int}";

// When a port is taken, the next ones are tried: 8080, 8081, 8082 ...
const PORT_TRIES: u16 = 20;

// Can be changed on the command line
static PREFERRED_PORTS: OnceLock<HashMap<Mode, u16>> = OnceLock::new();
// The ports that were actually opened
static OPEN_PORTS: LazyLock<Mutex<HashMap<Mode, u16>>> = LazyLock::new(Default::default);

pub fn set_ports(ports: HashMap<Mode, u16>) {
    let _ = PREFERRED_PORTS.set(ports);
}

pub fn preferred_port(mode: Mode) -> u16 {
    if let Some(port) = PREFERRED_PORTS.get().and_then(|p| p.get(&mode)) {
        return *port;
    }
    match mode {
        Mode::Thermal => 8080,
        Mode::Line => 8090,
        Mode::Image => 8070,
    }
}

// The port the server listens on, or the preferred one while it is not open yet
pub fn port(mode: Mode) -> u16 {
    OPEN_PORTS.lock().unwrap().get(&mode).copied().unwrap_or_else(|| preferred_port(mode))
}

pub fn address(mode: Mode) -> String {
    let ip = local_ip_address::local_ip().map_or("localhost".to_string(), |ip| ip.to_string());
    format!("ws://{ip}:{}", port(mode))
}

async fn bind(mode: Mode) -> Result<TcpListener, String> {
    let first = preferred_port(mode);
    let mut last_error = String::new();
    for port in (first..=u16::MAX).take(PORT_TRIES as usize) {
        match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(listener) => {
                OPEN_PORTS.lock().unwrap().insert(mode, port);
                return Ok(listener);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => last_error = e.to_string(),
            Err(e) => return Err(format!("Port {port}: {e}")),
        }
    }
    Err(format!("Ports {first} to {} are all in use ({last_error})", first.saturating_add(PORT_TRIES - 1)))
}

pub async fn serve(state: Shared, mode: Mode) {
    let listener = match bind(mode).await {
        Ok(listener) => listener,
        Err(e) => {
            let mut s = state.lock().unwrap();
            s.log(format!("{}: {e}", mode.label()));
            s.port_errors.insert(mode, e);
            return;
        }
    };
    if port(mode) != preferred_port(mode) {
        state.lock().unwrap().log(format!(
            "{}: port {} is in use, using {} instead. Change the port in the sketch.",
            mode.label(),
            preferred_port(mode),
            port(mode)
        ));
    }
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(connection(state.clone(), mode, stream));
    }
}

async fn connection(state: Shared, mode: Mode, stream: TcpStream) {
    let peer = stream.peer_addr().map_or("?".to_string(), |a| a.ip().to_string());
    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return };
    {
        let mut s = state.lock().unwrap();
        *s.clients.entry(mode).or_default() += 1;
        s.log(format!("{peer} connected to port {}", port(mode)));
    }

    // Page options of the CUPS protocol, per connection
    let mut options = PageOptions::default();
    // Like the Pi servers, the server only answers and never sends a greeting:
    // the sketches treat the next message as the answer to their request
    while let Some(Ok(message)) = ws.next().await {
        let reply = match message {
            Message::Text(text) => handle(&state, mode, Request::Text(text.to_string()), &mut options).await,
            Message::Binary(data) => handle(&state, mode, Request::Binary(data.to_vec()), &mut options).await,
            Message::Close(_) => break,
            _ => continue,
        };
        if ws.send(Message::text(reply)).await.is_err() {
            break;
        }
    }

    let mut s = state.lock().unwrap();
    *s.clients.entry(mode).or_default() -= 1;
    s.log(format!("{peer} disconnected from port {}", port(mode)));
}

enum Request {
    Text(String),
    Binary(Vec<u8>),
}

async fn handle(state: &Shared, mode: Mode, request: Request, options: &mut PageOptions) -> String {
    // Page options for the CUPS protocol work without a printer
    if let (Mode::Image, Request::Text(text)) = (mode, &request) {
        return match set_options(options, text) {
            Ok(()) => "ok".into(),
            Err(e) => format!("error: {e}"),
        };
    }

    let device = {
        let mut s = state.lock().unwrap();
        let Some(device) = s.active_device(mode) else {
            s.log(format!("Port {}: no printer selected for '{}'", port(mode), mode.label()));
            return format!("error: no printer selected for '{}'", mode.label());
        };
        // Same as on the Pi: one job at a time, others are answered with "busy"
        if !s.busy.insert(device.key.clone()) {
            return "busy".into();
        }
        device
    };

    let is_image = matches!(request, Request::Binary(_));
    let options = options.clone();
    let job_device = device.clone();
    let result = tokio::task::spawn_blocking(move || {
        let (job, what) = build_job(mode, request, &options)?;
        devices::send(&job_device, job).map(|()| what)
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));

    let mut s = state.lock().unwrap();
    s.busy.remove(&device.key);
    match result {
        Ok(what) => {
            s.log(format!("{what} → {}", device.name));
            // lp-websockets answers images with "printed_image"
            if mode == Mode::Line && is_image { "printed_image".into() } else { "ok".into() }
        }
        Err(e) => {
            s.log(format!("Error on {}: {e}", device.name));
            if mode == Mode::Line && is_image { "conversion_error".into() } else { format!("error: {e}") }
        }
    }
}

// Builds the printer data and a short description for the log
fn build_job(mode: Mode, request: Request, options: &PageOptions) -> Result<(Job, String), String> {
    match (mode, request) {
        (Mode::Thermal, Request::Text(text)) if text == "cut" => {
            Ok((Job::Raw { data: thermal::cut(), preview: None }, "Cut".into()))
        }
        (Mode::Thermal, Request::Text(text)) => {
            let json = parse_json(&text, THERMAL_FORMAT)?;
            let content = string(&json["text"]);
            let cut = json["cut"].as_bool().unwrap_or(false);
            let data = thermal::text(&content, int(&json["w"]), int(&json["h"]), &string(&json["style"]), cut);
            Ok((Job::Raw { data, preview: None }, format!("Text \"{}\"{}", short(&content), if cut { " + cut" } else { "" })))
        }
        (Mode::Thermal, Request::Binary(png)) => {
            let (data, dots) = thermal::image(&png)?;
            let preview = Some(bitmap::preview(&dots, thermal::preview_scale()));
            Ok((Job::Raw { data, preview }, format!("Image {}×{} dots", dots.width, dots.height)))
        }
        (Mode::Line, Request::Text(text)) => {
            let json = parse_json(&text, LINE_FORMAT)?;
            let content = string(&json["text"]);
            if !content.is_empty() {
                let data = line::text(&content, int(&json["w"]), int(&json["h"]), &string(&json["style"]));
                return Ok((Job::Raw { data, preview: None }, format!("Text \"{}\"", short(&content))));
            }
            match json["direction"].as_str() {
                Some(direction @ ("up" | "down")) => {
                    let steps = int(&json["steps"]).max(1);
                    let data = line::feed(direction == "up", steps);
                    Ok((Job::Raw { data, preview: None }, format!("Feed {direction} {steps}")))
                }
                _ => Err(format!("Nothing to print. {LINE_FORMAT}")),
            }
        }
        (Mode::Line, Request::Binary(png)) => {
            let (data, dots) = line::image(&png)?;
            let preview = Some(bitmap::preview(&dots, line::preview_scale()));
            Ok((Job::Raw { data, preview }, format!("Image {}×{} dots", dots.width, dots.height)))
        }
        (Mode::Image, Request::Binary(png)) => {
            let data = pdf::image_pdf(&png, options)?;
            let what = format!("Image on {}, {} cop{}", options.media, options.copies, if options.copies == 1 { "y" } else { "ies" });
            Ok((Job::Pdf { data, media: options.media.clone(), copies: options.copies }, what))
        }
        (Mode::Image, Request::Text(_)) => unreachable!("handled before"),
    }
}

fn set_options(options: &mut PageOptions, text: &str) -> Result<(), String> {
    let format = "Format: {media: 'A4', orientation: 'auto' | 'portrait' | 'landscape', fit: bool, dpi: number, margin: mm, copies: int, dither: bool}";
    let json = parse_json(text, format)?;
    // Merge the sent fields into the current options
    let mut merged = serde_json::to_value(&*options).map_err(|e| e.to_string())?;
    for (key, value) in json.as_object().ok_or(format)? {
        merged[key] = value.clone();
    }
    let new: PageOptions = serde_json::from_value(merged).map_err(|e| format!("{e}. {format}"))?;
    pdf::media_size(&new.media).ok_or(format!("Unknown media '{}'. Use A3, A4, A5, A6, Letter or Legal", new.media))?;
    *options = new;
    Ok(())
}

fn parse_json(text: &str, format: &str) -> Result<Value, String> {
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| format!("Could not parse JSON input. {format}"))
}

// Numbers may also arrive as strings or booleans from a sketch
fn int(value: &Value) -> i64 {
    match value {
        Value::Number(n) => n.as_f64().unwrap_or(0.0).round() as i64,
        Value::String(s) => s.trim().parse::<f64>().map_or(0, |n| n.round() as i64),
        Value::Bool(b) => *b as i64,
        _ => 0,
    }
}

fn string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn short(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.chars().count() > 30 || text.lines().count() > 1 {
        format!("{}…", line.chars().take(30).collect::<String>())
    } else {
        line.to_string()
    }
}

// Prints the server address on a printer, like the Pi servers do on start
pub fn test_job(device: &Device, mode: Mode) -> Job {
    let address = address(mode);
    match mode {
        Mode::Thermal => Job::Raw { data: thermal::text(&format!("{address}\n\n"), 1, 1, "bu", true), preview: None },
        Mode::Line => Job::Raw { data: line::text(&format!("{address}\n\n"), 1, 1, "bu"), preview: None },
        Mode::Image => Job::Text(format!("Printer Server\n\n{}\n\n{address}\n", device.name)),
    }
}
