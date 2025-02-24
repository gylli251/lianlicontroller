use hidapi::{HidApi, HidDevice};
use std::time::Duration;
use std::thread::sleep;
use thiserror::Error;
use clap::{Parser, ValueEnum};
use serde::Deserialize;
use std::fs;
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;

const VENDOR_ID: u16 = 0x0cf2;
const PRODUCT_ID: u16 = 0xa100;
const FAN_COUNT: u8 = 4;
const COLOR_BUFFER_SIZE: usize = 353;
const LEDS_PER_FAN: usize = 117;
const MIN_RPM: u16 = 805; // Adjusted from 800
const MAX_RPM: u16 = 1900;

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
    #[error("Invalid speed: {0} RPM (must be between {} and {})", MIN_RPM, MAX_RPM)]
    InvalidSpeed(u16),
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
#[command(about = "Control Lian Li fan colors, brightness, and speed (RPM or quiet modes)")]
struct Args {
    #[arg(long, help = "Red value (0-255)", default_value_t = 255)]
    red: u8,
    #[arg(long, help = "Green value (0-255)", default_value_t = 5)]
    green: u8,
    #[arg(long, help = "Blue value (0-255)", default_value_t = 5)]
    blue: u8,
    #[arg(long, help = "Brightness percentage (0-100)", default_value_t = 100.0)]
    brightness: f32,
    #[arg(long, help = "Fan speed in RPM (805-1900), ignored if mode is quiet-cpu or quiet-gpu", default_value_t = 1350)]
    speed: u16,
    #[arg(long, help = "Fan mode: fixed, quiet-cpu, quiet-gpu", default_value = "fixed")]
    mode: FanMode,
    #[arg(long, help = "Path to config file (overrides CLI args if present)")]
    config: Option<String>,
}

#[derive(Deserialize, Debug)]
struct Config {
    #[serde(default)]
    red: Option<u8>,
    #[serde(default)]
    green: Option<u8>,
    #[serde(default)]
    blue: Option<u8>,
    #[serde(default)]
    color: Option<String>,
    brightness: f32,
    #[serde(default = "default_speed")]
    speed: u16,
    #[serde(default)]
    mode: FanMode,
}

fn default_speed() -> u16 { 1350 }

struct FanController {
    device: HidDevice,
}

impl FanController {
    fn open() -> Result<Self, FanControlError> {
        let api = HidApi::new()?;
        match api.open(VENDOR_ID, PRODUCT_ID) {
            Ok(device) => {
                println!("Connected to device VID:{:04x} PID:{:04x}", VENDOR_ID, PRODUCT_ID);
                Ok(FanController { device })
            }
            Err(e) => {
                if e.to_string().contains("device not found") {
                    Err(FanControlError::DeviceNotFound)
                } else {
                    Err(FanControlError::HidApi(e))
                }
            }
        }
    }

    fn send_init(&self) -> Result<(), FanControlError> {
        let init_commands = [
            [0xe0, 0x50, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [0xe0, 0x10, 0x32, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [0xe0, 0x10, 0x32, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [0xe0, 0x10, 0x32, 0x23, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [0xe0, 0x10, 0x32, 0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        ];

        for cmd in &init_commands {
            self.device.send_feature_report(cmd)?;
            println!("Sent init command: {:02x?}", cmd);
            sleep(Duration::from_millis(100));
        }

        let mut buf = [0u8; 65];
        buf[0] = 0xe0;
        match self.device.get_feature_report(&mut buf) {
            Ok(bytes_read) => println!("Read {} bytes after init: {:02x?}", bytes_read, &buf[..bytes_read]),
            Err(e) => println!("Failed to read feature report: {}. Skipping...", e),
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

        let mut buf = vec![0xe0, 0x30 + fan];
        let brightness_factor = brightness / 100.0;
        let scaled_r = (r as f32 * brightness_factor).min(255.0) as u8;
        let scaled_g = (g as f32 * brightness_factor).min(255.0) as u8;
        let scaled_b = (b as f32 * brightness_factor).min(255.0) as u8;

        let colors = [scaled_r, scaled_b, scaled_g]; // Hardcoded to RBG order (Red, Blue, Green)

        for _ in 0..LEDS_PER_FAN {
            buf.extend_from_slice(&colors);
        }
        buf.resize(COLOR_BUFFER_SIZE, 0x00);

        self.device.write(&buf)?;
        println!(
            "Set fan {} to RGB({},{},{}) at {:.0}% brightness",
            fan, scaled_r, scaled_g, scaled_b, brightness
        );
        sleep(Duration::from_millis(100));

        let confirm_cmds = [
            [0xe0, 0x10, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [0xe0, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            [0xe0, 0x60, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        ];
        for cmd in &confirm_cmds {
            self.device.send_feature_report(cmd)?;
            println!("Sent confirmation command: {:02x?}", cmd);
            sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    fn set_fan_speed(&self, fan: u8, speed: u16) -> Result<(), FanControlError> {
        if fan >= FAN_COUNT {
            return Err(FanControlError::InvalidFan(fan));
        }
        let clamped_speed = speed.clamp(MIN_RPM, MAX_RPM);
        if speed != clamped_speed {
            return Err(FanControlError::InvalidSpeed(speed));
        }

        // Disable PWM mode (set to Manual mode) for this fan
        let channel_byte = 0x10 << fan;
        self.device.write(&[0xe0, 0x10, 0x31, channel_byte])?;
        println!("Set fan {} to Manual mode", fan);
        sleep(Duration::from_millis(200));

        // Convert RPM to speed byte (805-1900 RPM maps to 0-255)
        let speed_range = (MAX_RPM - MIN_RPM) as f32; // 1095
        let speed_value = clamped_speed - MIN_RPM; // Offset from minimum
        let speed_byte = ((speed_value as f32 / speed_range) * 255.0).min(255.0) as u8;
        self.device.write(&[0xe0, (fan + 32) as u8, 0x00, speed_byte])?;
        println!("Set fan {} speed to {} RPM", fan, clamped_speed);
        sleep(Duration::from_millis(100));

        Ok(())
    }

    fn set_all_fans(&self, r: u8, g: u8, b: u8, brightness: f32, speed: u16, mode: FanMode) -> Result<(), FanControlError> {
        for fan in 0..FAN_COUNT {
            self.set_fan_color(fan, r, g, b, brightness)?;
            match mode {
                FanMode::Fixed => {
                    self.set_fan_speed(fan, speed)?;
                }
                FanMode::QuietCpu => {
                    let cpu_temp = get_cpu_temp()?;
                    let rpm = map_temp_to_rpm(cpu_temp);
                    self.set_fan_speed(fan, rpm)?;
                    println!("Fan {} synced to CPU temp {:.1}°C -> {} RPM", fan, cpu_temp, rpm);
                }
                FanMode::QuietGpu => {
                    let gpu_temp = get_gpu_temp()?;
                    let rpm = map_temp_to_rpm(gpu_temp);
                    self.set_fan_speed(fan, rpm)?;
                    println!("Fan {} synced to GPU temp {:.1}°C -> {} RPM", fan, gpu_temp, rpm);
                }
            }
            sleep(Duration::from_millis(200));
        }
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

// Map temperature to RPM for quiet modes (linear: 30°C -> 805 RPM, 80°C -> 1900 RPM)
fn map_temp_to_rpm(temp: f32) -> u16 {
    let temp = temp.clamp(30.0, 80.0);
    let temp_range = 80.0 - 30.0; // 50°C
    let rpm_range = (MAX_RPM - MIN_RPM) as f32; // 1095 RPM
    let rpm = MIN_RPM as f32 + ((temp - 30.0) / temp_range) * rpm_range;
    rpm.round() as u16
}

// Get CPU temperature from /sys/class/thermal (Linux only)
fn get_cpu_temp() -> Result<f32, FanControlError> {
    for zone in 0..=9 { // Check thermal_zone0 to thermal_zone9
        let temp_path = format!("/sys/class/thermal/thermal_zone{}/temp", zone);
        if let Ok(temp_str) = fs::read_to_string(&temp_path) {
            if let Ok(temp_millidegrees) = temp_str.trim().parse::<i32>() {
                let temp = temp_millidegrees as f32 / 1000.0; // Convert millidegrees to degrees
                println!("Detected CPU temperature: {}°C from {}", temp, temp_path);
                return Ok(temp);
            }
        }
    }
    println!("No CPU temperature detected, using fallback 50°C");
    Ok(50.0) // Fallback if no temp available
}

// Detect GPU type and read temperature (Linux only)
fn get_gpu_temp() -> Result<f32, FanControlError> {
    // Try NVIDIA via NVML
    if let Ok(nvml) = Nvml::init() {
        if let Ok(device) = nvml.device_by_index(0) {
            let temp = device.temperature(TemperatureSensor::Gpu)?;
            println!("Detected NVIDIA GPU, temperature: {}°C", temp);
            return Ok(temp as f32);
        }
    }

    // Try AMD via /sys/class/drm
    for card in 0..=4 {
        let temp_path = format!("/sys/class/drm/card{}/device/hwmon/hwmon*/temp1_input", card);
        if let Ok(entries) = glob::glob(&temp_path) {
            for entry in entries.flatten() {
                if let Ok(temp_str) = fs::read_to_string(&entry) {
                    if let Ok(temp_millidegrees) = temp_str.trim().parse::<i32>() {
                        let temp = temp_millidegrees as f32 / 1000.0;
                        println!("Detected AMD GPU, temperature: {}°C", temp);
                        return Ok(temp);
                    }
                }
            }
        }
    }

    println!("No GPU temperature detected, using fallback 50°C");
    Ok(50.0) // Fallback if no GPU temp available
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let (r, g, b, brightness, speed, mode) = match args.config {
        Some(ref config_path) => {
            let contents = fs::read_to_string(config_path)?;
            let config: Config = toml::from_str(&contents)?;
            let (r, g, b) = match config.color {
                Some(hex) => parse_hex_color(&hex)?,
                None => (
                    config.red.unwrap_or(255),
                    config.green.unwrap_or(5),
                    config.blue.unwrap_or(5),
                ),
            };
            (r, g, b, config.brightness, config.speed, config.mode)
        }
        None => (args.red, args.green, args.blue, args.brightness, args.speed, args.mode),
    };

    let controller = FanController::open()?;
    controller.send_init()?;
    controller.set_all_fans(r, g, b, brightness, speed, mode)?;
    sleep(Duration::from_millis(100));
    Ok(())
}