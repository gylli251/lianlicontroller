# Lian Li Fan Controller

A Rust-based daemon for controlling Lian Li UNI FAN SL-INF fans (VID: 0x0cf2, PID: 0xa100). Manage RGB lighting, fan speeds, and create temperature-based profiles through CLI or a config file.

[![Rust](https://img.shields.io/badge/Rust-1.60%2B-blue?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

## Features

- 🎨 Set RGB colors and brightness for all fans
- 🌀 Control fan speeds in RPM (805-1900 range)
- 🌡️ Temperature-based speed control modes:
  - Quiet CPU Mode: Dynamically adjusts speeds based on CPU temperature
  - Quiet GPU Mode: Syncs fan speeds with GPU temperature
- Configuration file support (TOML format)
- Systemd service integration for background operation
- Automatic detection of NVIDIA/AMD GPU temperatures

--------------------------------------------------

```
# Installation

### Prerequisites

- Rust 1.60+
- libusb development files
- Systemd (Linux only)

    # Example for Ubuntu/Debian
    sudo apt update
    sudo apt install build-essential libudev-dev libusb-1.0-0-dev

    # Clone and install
    git clone https://github.com/yourusername/lian-li-fan-controller.git
    cd lian-li-fan-controller
    ./install.sh
```

--------------------------------------------------

```
# Configuration

### Config File

By default, the daemon reads /etc/lianlicontroller/fans.toml:

    color = "#FF0505"    # Hex color code
    brightness = 100.0   # 0-100%
    speed = 1350         # 805-1900 RPM (only used if mode = "fixed")
    mode = "quietgpu"    # Options: fixed, quietcpu, quietgpu
```

--------------------------------------------------

```
# CLI Options

    lianlicontroller \
      --red 255 \
      --green 5 \
      --blue 5 \
      --brightness 100 \
      --speed 1350 \
      --mode quietgpu \
      --config /path/to/config.toml

--red, --green, --blue (0-255): Color components
--brightness (0-100): RGB brightness percentage
--speed (805-1900): Target RPM (if mode is fixed)
--mode: fixed | quietcpu | quietgpu
--config: Provide a specific TOML config file
```

--------------------------------------------------

```
# Usage

### Service Management

    # Start service
    sudo systemctl start lianlicontroller

    # Enable auto-start at boot
    sudo systemctl enable lianlicontroller

    # Check status
    systemctl status lianlicontroller

    # View logs
    journalctl -u lianlicontroller -f
```

--------------------------------------------------

```
# Example Scenarios

### Fixed Color/Speed Mode

    lianlicontroller --red 0 --green 255 --blue 0 --brightness 75 --speed 1200 --mode fixed

### CPU Temperature-Based Control

    # fans.toml
    color = "#00FF00"
    brightness = 50
    mode = "quietcpu"
```

--------------------------------------------------

```
# Troubleshooting

### Device Not Found

- Ensure fans are connected and powered.
- Check USB permissions:

    echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="0cf2", ATTR{idProduct}=="a100", MODE="0666"' | sudo tee /etc/udev/rules.d/99-lianli.rules
    sudo udevadm control --reload-rules

### Invalid Configuration

- Validate TOML syntax with tools like tomlv.
- Ensure color values are valid hex codes.
- Verify RPM values are within the 805-1900 range.
```

--------------------------------------------------

```
# Uninstallation

    sudo systemctl stop lianlicontroller
    sudo systemctl disable lianlicontroller
    sudo rm /usr/local/bin/lianlicontroller \
             /etc/systemd/system/lianlicontroller.service \
             /etc/lianlicontroller/fans.toml
    sudo systemctl daemon-reload
```

--------------------------------------------------

```
# License

MIT License – see LICENSE for details.
```

--------------------------------------------------

```
# Acknowledgments

- hidapi-rs for USB HID communication
- NVML Wrapper for NVIDIA GPU monitoring
- sysinfo for system temperature data
```
