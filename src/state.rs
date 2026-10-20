// State shared between the GUI, the device scan and the WebSocket servers
use crate::devices::{Device, Mode};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const LOG_LINES: usize = 200;

pub type Shared = Arc<Mutex<State>>;

// Saved between launches
#[derive(Default, Serialize, Deserialize)]
pub struct Settings {
    // Mode chosen per device key
    pub modes: HashMap<String, Mode>,
    // Active device key per mode: one printer each for the thermal, line and CUPS port
    pub active: HashMap<Mode, String>,
}

#[derive(Default)]
pub struct State {
    pub devices: Vec<Device>,
    pub settings: Settings,
    // Keys of the devices that are printing right now
    pub busy: HashSet<String>,
    // Connected sketches per mode
    pub clients: HashMap<Mode, usize>,
    // Ports that could not be opened, e.g. because another program uses them
    pub port_errors: HashMap<Mode, String>,
    pub log: VecDeque<String>,
}

impl State {
    pub fn load() -> State {
        State { settings: Settings::read(), ..Default::default() }
    }

    // Changes are made on the saved settings, so a second running app cannot undo them
    fn change(&mut self, change: impl FnOnce(&mut State)) {
        self.settings = Settings::read();
        change(self);
        self.settings.write();
    }

    pub fn mode(&self, device: &Device) -> Mode {
        self.settings
            .modes
            .get(&device.key)
            .copied()
            .filter(|m| device.modes().contains(m))
            .unwrap_or_else(|| device.default_mode())
    }

    pub fn is_active(&self, device: &Device) -> bool {
        self.settings.active.get(&self.mode(device)) == Some(&device.key)
    }

    // Checking a printer replaces the active printer of the same mode
    pub fn set_active(&mut self, device: &Device, active: bool) {
        self.change(|s| {
            let mode = s.mode(device);
            if active {
                s.settings.active.insert(mode, device.key.clone());
            } else if s.is_active(device) {
                s.settings.active.remove(&mode);
            }
        });
    }

    pub fn set_mode(&mut self, device: &Device, mode: Mode) {
        self.change(|s| {
            let was_active = s.is_active(device);
            if was_active {
                s.settings.active.remove(&s.mode(device));
            }
            s.settings.modes.insert(device.key.clone(), mode);
            if was_active {
                s.settings.active.insert(mode, device.key.clone());
            }
        });
    }

    // The connected printer that is active for this mode
    pub fn active_device(&self, mode: Mode) -> Option<Device> {
        let key = self.settings.active.get(&mode)?;
        self.devices.iter().find(|d| &d.key == key && self.mode(d) == mode).cloned()
    }

    pub fn log(&mut self, message: impl Into<String>) {
        let time = chrono::Local::now().format("%H:%M:%S");
        self.log.push_back(format!("{time}  {}", message.into()));
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }
}

impl Settings {
    fn read() -> Settings {
        let path = settings_path();
        let Ok(data) = std::fs::read(&path) else { return Settings::default() };
        match serde_json::from_slice::<Settings>(&data) {
            Ok(mut settings) => {
                settings.migrate();
                settings
            }
            Err(_) => {
                // Keep a damaged file instead of overwriting it with empty settings
                let _ = std::fs::rename(&path, path.with_extension("json.damaged"));
                Settings::default()
            }
        }
    }

    // Written to a temporary file first, so the settings are never half written
    fn write(&self) {
        let path = settings_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let temporary = path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_vec_pretty(self)
            && std::fs::write(&temporary, json).is_ok()
        {
            let _ = std::fs::rename(&temporary, &path);
        }
    }

    // USB keys used to contain the port (usb:04b8:0e02:2-1.2), now they are usb:04b8:0e02
    fn migrate(&mut self) {
        let new_key = |key: &str| match key.strip_prefix("usb:").map(|k| k.split(':').count()) {
            Some(3) => key.rsplit_once(':').map_or(key, |(k, _)| k).to_string(),
            _ => key.to_string(),
        };
        self.modes = self.modes.drain().map(|(key, mode)| (new_key(&key), mode)).collect();
        for key in self.active.values_mut() {
            *key = new_key(key);
        }
    }
}

fn settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("printer-server")
        .join("settings.json")
}

