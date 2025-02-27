use hidapi::{HidApi, HidDevice};
use std::time::Duration;
use std::thread::sleep;
use thiserror::Error;
use clap::{Parser, ValueEnum};
use serde::Deserialize;
use std::fs;
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use sysinfo::Components;
use log::{info, error, warn, debug, LevelFilter};
use env_logger::Builder;

// Constants
const VENDOR_ID: u16 = 0x0cf2;
const SUPPORTED_PRODUCT_IDS: [u16; 7] = [0x7750, 0xa100, 0xa101, 0xa102, 0xa103, 0xa104, 0xa105];
const FAN_COUNT: u8 = 4;
const COLOR_BUFFER_SIZE: usize = 353;
const LEDS_PER_FAN: usize = 117;
const REPORT_ID: u8 = 0xe0;
const SET_COLOR_CMD_BASE: u8 = 0x30;
const SET_SPEED_CMD: u8 = 0x10;

#[derive(Error, Debug)]
enum FanControlError {
    #[error("HID API error: {0}")]
    HidApi(#[from] hidapi::HidError),
    #[error("Device not found")]
    DeviceNotFound,
    #[error("Invalid fan number: {0}")]
    InvalidFan(u8),
    #[error("Invalid brightness: {0}")]
    InvalidBrightness(f32),
    #[error("Invalid speed: {0} RPM (must be between {1} and {2})")]
    InvalidSpeed(u16, u16, u16),
    #[error("Config file error: {0}")]
    ConfigError(#[from] std::io::Error),
    #[error("TOML parsing error: {0}")]
    TomlError(#[from] toml::de::Error),
    #[error("Invalid hex color: {0}")]
    InvalidHexColor(String),
    #[error("NVML error: {0}")]
    NvmlError(#[from] nvml_wrapper::error::NvmlError),
}

#[derive(ValueEnum, Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum FanMode {
    Fixed,
    QuietCpu,
    QuietGpu,
}

impl Default for FanMode {
    fn default() -> Self {
        FanMode::Fixed
    }
}

#[derive(Parser, Debug)]
#[command(about = "Control Lian Li fan colors, brightness, and speed")]
struct Args {
    #[arg(long, help = "Red value (0-255)", default_value_t = 255)]
    red: u8,
    #[arg(long, help = "Green value (0-255)", default_value_t = 5)]
    green: u8,
    #[arg(long, help = "Blue value (0-255)", default_value_t = 5)]
    blue: u8,
    #[arg(long, help = "Brightness percentage (0-100)", default_value_t = 100.0)]
    brightness: f32,
    #[arg(long, help = "Fan speed in RPM", default_value_t = 1350)]
    speed: u16,
    #[arg(long, help = "Fan mode: fixed, quiet-cpu, quiet-gpu", default_value = "fixed")]
    mode: FanMode,
    #[arg(long, help = "Path to config file", default_value = "/etc/lianlicontroller/fans.toml")]
    config: String,
    #[arg(long, help = "Log level (error, warn, info, debug, trace)", default_value = "info")]
    log_level: Option<String>,
}

#[derive(Deserialize, Debug)]
struct GlobalConfig {
    color: Option<String>,
    red: Option<u8>,
    green: Option<u8>,
    blue: Option<u8>,
    brightness: Option<f32>,
    speed: Option<u16>,
    mode: Option<FanMode>,
    log_level: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ZoneConfig {
    enabled: Option<bool>,
    color: Option<String>,
    red: Option<u8>,
    green: Option<u8>,
    blue: Option<u8>,
    brightness: Option<f32>,
    speed: Option<u16>,
    mode: Option<FanMode>,
}

#[derive(Deserialize, Debug)]
struct Config {
    global: Option<GlobalConfig>,
    zone_0: Option<ZoneConfig>,
    zone_1: Option<ZoneConfig>,
    zone_2: Option<ZoneConfig>,
    zone_3: Option<ZoneConfig>,
}

struct EffectiveZoneSettings {
    r: u8,
    g: u8,
    b: u8,
    brightness: f32,
    speed: u16,
    mode: FanMode,
}

struct ModelConfig {
    mode_byte: u8,
    sync_byte: u8,
    min_rpm: u16,
    max_rpm: u16,
}

fn get_model_config(product_id: u16) -> ModelConfig {
    match product_id {
        0xa100 | 0x7750 => ModelConfig { mode_byte: 49, sync_byte: 48, min_rpm: 800, max_rpm: 1900 },
        0xa101 => ModelConfig { mode_byte: 66, sync_byte: 65, min_rpm: 800, max_rpm: 1900 },
        0xa102 => ModelConfig { mode_byte: 98, sync_byte: 97, min_rpm: 200, max_rpm: 2100 },
        0xa103 | 0xa105 => ModelConfig { mode_byte: 98, sync_byte: 97, min_rpm: 250, max_rpm: 2000 },
        0xa104 => ModelConfig { mode_byte: 98, sync_byte: 97, min_rpm: 250, max_rpm: 2000 },
        _ => ModelConfig { mode_byte: 49, sync_byte: 48, min_rpm: 800, max_rpm: 1900 },
    }
}

struct FanController {
    device: HidDevice,
    product_id: u16,
    model_config: ModelConfig,
}

impl FanController {
    fn open() -> Result<Self, FanControlError> {
        let api = HidApi::new()?;
        for &pid in &SUPPORTED_PRODUCT_IDS {
            debug!("Attempting to open device VID:{:04x} PID:{:04x}", VENDOR_ID, pid);
            match api.open(VENDOR_ID, pid) {
                Ok(device) => {
                    let model_config = get_model_config(pid);
                    info!("Connected to device VID:{:04x} PID:{:04x} (RPM range: {}-{})",
                          VENDOR_ID, pid, model_config.min_rpm, model_config.max_rpm);
                    return Ok(FanController { device, product_id: pid, model_config });
                },
                Err(e) => {
                    warn!("Failed to open device VID:{:04x} PID:{:04x}: {}", VENDOR_ID, pid, e);
                    continue;
                },
            }
        }
        error!("No supported device found");
        Err(FanControlError::DeviceNotFound)
    }

    fn send_init(&self) -> Result<(), FanControlError> {
        let init_commands = [
            [REPORT_ID, 0x50, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [REPORT_ID, SET_SPEED_CMD, 0x32, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [REPORT_ID, SET_SPEED_CMD, 0x32, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [REPORT_ID, SET_SPEED_CMD, 0x32, 0x23, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [REPORT_ID, SET_SPEED_CMD, 0x32, 0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        ];

        info!("Initializing device");
        for cmd in &init_commands {
            debug!("Sending init command: {:02x?}", cmd);
            self.device.send_feature_report(cmd)?;
            sleep(Duration::from_millis(100));
        }

        let mut buf = [0u8; 65];
        buf[0] = REPORT_ID;
        match self.device.get_feature_report(&mut buf) {
            Ok(bytes_read) => debug!("Read {} bytes after init: {:02x?}", bytes_read, &buf[..bytes_read]),
            Err(e) => warn!("Failed to read feature report: {}. Skipping...", e),
        }
        sleep(Duration::from_millis(100));
        Ok(())
    }

    fn set_fan_color(&self, fan: u8, r: u8, g: u8, b: u8, brightness: f32) -> Result<(), FanControlError> {
        if fan >= FAN_COUNT {
            return Err(FanControlError::InvalidFan(fan));
        }
        if !(0.0..=100.0).contains(&brightness) {
            return Err(FanControlError::InvalidBrightness(brightness));
        }

        let mut buf = vec![REPORT_ID, SET_COLOR_CMD_BASE + fan];
        let brightness_factor = brightness / 100.0;
        let scaled_r = (r as f32 * brightness_factor).min(255.0) as u8;
        let scaled_g = (g as f32 * brightness_factor).min(255.0) as u8;
        let scaled_b = (b as f32 * brightness_factor).min(255.0) as u8;

        let colors = [scaled_r, scaled_b, scaled_g]; // RBG order for Lian Li
        for _ in 0..LEDS_PER_FAN {
            buf.extend_from_slice(&colors);
        }
        buf.resize(COLOR_BUFFER_SIZE, 0x00);

        debug!("Setting zone {} to RGB({},{},{}) at {:.0}% brightness", fan, scaled_r, scaled_g, scaled_b, brightness);
        self.device.write(&buf)?;
        sleep(Duration::from_millis(100));

        let confirm_cmds = [
            [REPORT_ID, SET_SPEED_CMD, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [REPORT_ID, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [REPORT_ID, 0x60, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        ];
        for cmd in &confirm_cmds {
            debug!("Sending confirmation command: {:02x?}", cmd);
            self.device.send_feature_report(cmd)?;
            sleep(Duration::from_millis(50));
        }
        info!("Set zone {} to RGB({},{},{}) at {:.0}% brightness", fan, scaled_r, scaled_g, scaled_b, brightness);
        Ok(())
    }

    fn set_fan_speed(&self, fan: u8, speed: u16) -> Result<(), FanControlError> {
        if fan >= FAN_COUNT {
            return Err(FanControlError::InvalidFan(fan));
        }
        let min_rpm = self.model_config.min_rpm;
        let max_rpm = self.model_config.max_rpm;
        if speed < min_rpm || speed > max_rpm {
            return Err(FanControlError::InvalidSpeed(speed, min_rpm, max_rpm));
        }

        let channel_byte = 0x10 << fan;
        debug!("Setting zone {} to Manual mode", fan);
        self.device.write(&[REPORT_ID, SET_SPEED_CMD, self.model_config.mode_byte, channel_byte])?;
        sleep(Duration::from_millis(200));

        let speed_range = (max_rpm - min_rpm) as f32;
        let speed_value = (speed - min_rpm) as f32;
        let speed_byte = if speed_range > 0.0 {
            1 + ((speed_value / speed_range * 254.0).round() as u8)
        } else {
            1
        };
        debug!("Setting zone {} speed to {} RPM (byte: {})", fan, speed, speed_byte);
        self.device.write(&[REPORT_ID, (fan + 32) as u8, 0x00, speed_byte])?;
        sleep(Duration::from_millis(100));
        info!("Set zone {} speed to {} RPM", fan, speed);
        Ok(())
    }
}

fn parse_hex_color(hex: &str) -> Result<(u8, u8, u8), FanControlError> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return Err(FanControlError::InvalidHexColor(hex.to_string()));
    }
    let r = u8::from_str_radix(&hex[0..2], 16)
        .map_err(|_| FanControlError::InvalidHexColor(hex.to_string()))?;
    let g = u8::from_str_radix(&hex[2..4], 16)
        .map_err(|_| FanControlError::InvalidHexColor(hex.to_string()))?;
    let b = u8::from_str_radix(&hex[4..6], 16)
        .map_err(|_| FanControlError::InvalidHexColor(hex.to_string()))?;
    Ok((r, g, b))
}

fn map_temp_to_rpm(temp: f32, min_rpm: u16, max_rpm: u16) -> u16 {
    if temp <= 60.0 {
        min_rpm
    } else if temp >= 95.0 {
        max_rpm
    } else {
        let temp_range = 95.0 - 60.0;
        let rpm_range = max_rpm - min_rpm;
        let rpm = min_rpm as f32 + ((temp - 60.0) / temp_range) * rpm_range as f32;
        rpm.round() as u16
    }
}

fn get_cpu_temp() -> Result<f32, FanControlError> {
    let mut components = Components::new_with_refreshed_list();
    components.refresh(true);

    let cpu_labels = ["tctl", "cpu", "core", "package", "tdie", "smbusmaster"];
    let mut cpu_temps: Vec<f32> = Vec::new();
    for component in components.iter() {
        let label = component.label().to_lowercase();
        for &cpu_label in &cpu_labels {
            if label.contains(cpu_label) {
                if let Some(temp) = component.temperature() {
                    if temp > 0.0 {
                        cpu_temps.push(temp);
                    }
                }
            }
        }
    }

    if !cpu_temps.is_empty() {
        let max_temp = cpu_temps.iter().fold(f32::MIN, |a, &b| a.max(b));
        info!("Detected CPU temperature: {:.1}°C", max_temp);
        Ok(max_temp)
    } else {
        let max_temp = components.iter()
            .filter_map(|c| c.temperature())
            .filter(|&t| t > 0.0)
            .fold(f32::MIN, |a, b| a.max(b));
        if max_temp > f32::MIN {
            info!("No CPU sensors found; using highest temp: {:.1}°C", max_temp);
            Ok(max_temp)
        } else {
            warn!("No valid temperatures detected; defaulting to 50°C");
            Ok(50.0)
        }
    }
}

fn get_gpu_temp() -> Result<f32, FanControlError> {
    if let Ok(nvml) = Nvml::init() {
        if let Ok(device) = nvml.device_by_index(0) {
            let temp = device.temperature(TemperatureSensor::Gpu)?;
            info!("Detected NVIDIA GPU, temperature: {}°C", temp);
            return Ok(temp as f32);
        }
    }

    for card in 0..=4 {
        let temp_path = format!("/sys/class/drm/card{}/device/hwmon/hwmon*/temp1_input", card);
        if let Ok(entries) = glob::glob(&temp_path) {
            for entry in entries.flatten() {
                if let Ok(temp_str) = fs::read_to_string(&entry) {
                    if let Ok(temp_millidegrees) = temp_str.trim().parse::<i32>() {
                        let temp = temp_millidegrees as f32 / 1000.0;
                        info!("Detected AMD GPU, temperature: {}°C", temp);
                        return Ok(temp);
                    }
                }
            }
        }
    }

    warn!("No GPU temperature detected, using fallback 50°C");
    Ok(50.0)
}

fn get_effective_settings(
    zone_config: Option<&ZoneConfig>,
    global_config: Option<&GlobalConfig>,
    args: &Args,
    model_config: &ModelConfig,
    _zone_num: u8,
) -> EffectiveZoneSettings {
    let enabled = zone_config.and_then(|z| z.enabled).unwrap_or(true);
    if !enabled {
        return EffectiveZoneSettings {
            r: 0,
            g: 0,
            b: 0,
            brightness: 0.0,
            speed: model_config.min_rpm,
            mode: FanMode::Fixed,
        };
    }
    let (r, g, b) = get_rgb(zone_config, global_config, args);
    let brightness = get_field(zone_config, global_config, args.brightness, |z| z.brightness, |g| g.brightness);
    let speed = get_field(zone_config, global_config, args.speed, |z| z.speed, |g| g.speed);
    let mode = get_field(zone_config, global_config, args.mode.clone(), |z| z.mode.clone(), |g| g.mode.clone());
    EffectiveZoneSettings { r, g, b, brightness, speed, mode }
}

fn get_rgb(
    zone_config: Option<&ZoneConfig>,
    global_config: Option<&GlobalConfig>,
    args: &Args,
) -> (u8, u8, u8) {
    if let Some(zone) = zone_config {
        if let Some(color) = &zone.color {
            if let Ok(rgb) = parse_hex_color(color) {
                return rgb;
            }
        }
        if let (Some(r), Some(g), Some(b)) = (zone.red, zone.green, zone.blue) {
            return (r, g, b);
        }
    }
    if let Some(global) = global_config {
        if let Some(color) = &global.color {
            if let Ok(rgb) = parse_hex_color(color) {
                return rgb;
            }
        }
        if let (Some(r), Some(g), Some(b)) = (global.red, global.green, global.blue) {
            return (r, g, b);
        }
    }
    (args.red, args.green, args.blue)
}

fn get_field<T, F, G>(
    zone_config: Option<&ZoneConfig>,
    global_config: Option<&GlobalConfig>,
    default: T,
    zone_fn: F,
    global_fn: G,
) -> T
where
    T: Clone,
    F: Fn(&ZoneConfig) -> Option<T>,
    G: Fn(&GlobalConfig) -> Option<T>,
{
    zone_config.and_then(|z| zone_fn(z)).or_else(|| global_config.and_then(|g| global_fn(g))).unwrap_or(default)
}

fn get_zone_config(config: &Option<Config>, zone_num: u8) -> Option<&ZoneConfig> {
    config.as_ref().and_then(|c| match zone_num {
        0 => c.zone_0.as_ref(),
        1 => c.zone_1.as_ref(),
        2 => c.zone_2.as_ref(),
        3 => c.zone_3.as_ref(),
        _ => None,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Parse the config file
    let config: Option<Config> = match fs::read_to_string(&args.config) {
        Ok(contents) => {
            match toml::from_str(&contents) {
                Ok(config) => Some(config),
                Err(e) => {
                    eprintln!("Failed to parse config file '{}': {}. Using CLI defaults.", args.config, e);
                    None
                }
            }
        }
        Err(e) => {
            eprintln!("No config file found at '{}': {}. Using CLI defaults.", args.config, e);
            None
        }
    };

    // Set log level with precedence: CLI > global config > default
    let log_level_str = args.log_level.clone()
        .or(config.as_ref().and_then(|c| c.global.as_ref()).and_then(|g| g.log_level.clone()))
        .unwrap_or_else(|| "info".to_string());
    let log_level = match log_level_str.as_str() {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "info" => LevelFilter::Info,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => {
            eprintln!("Invalid log level '{}', defaulting to 'info'", log_level_str);
            LevelFilter::Info
        }
    };
    Builder::new().filter_level(log_level).init();

    // Open the fan controller
    let controller = FanController::open()?;
    if let Err(e) = controller.send_init() {
        error!("Failed to initialize device: {}", e);
        return Err(e.into());
    }

    // Set colors for all zones
    for zone_num in 0..FAN_COUNT {
        let zone_config = get_zone_config(&config, zone_num);
        let effective_settings = get_effective_settings(
            zone_config,
            config.as_ref().and_then(|c| c.global.as_ref()),
            &args,
            &controller.model_config,
            zone_num,
        );
        controller.set_fan_color(
            zone_num,
            effective_settings.r,
            effective_settings.g,
            effective_settings.b,
            effective_settings.brightness,
        )?;
    }

    // Set speeds for fixed mode zones
    for zone_num in 0..FAN_COUNT {
        let zone_config = get_zone_config(&config, zone_num);
        let effective_settings = get_effective_settings(
            zone_config,
            config.as_ref().and_then(|c| c.global.as_ref()),
            &args,
            &controller.model_config,
            zone_num,
        );
        if effective_settings.mode == FanMode::Fixed {
            controller.set_fan_speed(zone_num, effective_settings.speed)?;
        }
    }

    // Collect dynamic zones (QuietCpu or QuietGpu)
    let dynamic_zones: Vec<(u8, EffectiveZoneSettings)> = (0..FAN_COUNT)
        .filter_map(|zone_num| {
            let zone_config = get_zone_config(&config, zone_num);
            let settings = get_effective_settings(
                zone_config,
                config.as_ref().and_then(|c| c.global.as_ref()),
                &args,
                &controller.model_config,
                zone_num,
            );
            if matches!(settings.mode, FanMode::QuietCpu | FanMode::QuietGpu) {
                Some((zone_num, settings))
            } else {
                None
            }
        })
        .collect();

    // If there are dynamic zones, enter a loop to update their speeds
    if !dynamic_zones.is_empty() {
        info!("Entering dynamic mode loop for zones: {:?}", dynamic_zones.iter().map(|(z, _)| z).collect::<Vec<_>>());
        loop {
            for (zone_num, settings) in &dynamic_zones {
                let temp = match settings.mode {
                    FanMode::QuietCpu => get_cpu_temp()?,
                    FanMode::QuietGpu => get_gpu_temp()?,
                    _ => unreachable!(),
                };
                let rpm = map_temp_to_rpm(temp, controller.model_config.min_rpm, controller.model_config.max_rpm);
                controller.set_fan_speed(*zone_num, rpm)?;
            }
            sleep(Duration::from_secs(5));
        }
    }

    Ok(())
}