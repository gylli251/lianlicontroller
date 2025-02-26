#!/bin/bash

# Exit on any error
set -e

# Build the project in release mode
echo "Building the project..."
cargo build --release

# Install the binary to /usr/local/bin
echo "Installing binary to /usr/local/bin/lianlicontroller..."
sudo cp target/release/lianlicontroller /usr/local/bin/lianlicontroller

# Create the config directory if it doesn't exist
echo "Creating config directory /etc/lianlicontroller..."
sudo mkdir -p /etc/lianlicontroller

# Check if fans.toml exists in the repo and copy it, or warn if missing
if [ -f fans.toml ]; then
    echo "Copying fans.toml from repo to /etc/lianlicontroller/fans.toml..."
    sudo cp fans.toml /etc/lianlicontroller/fans.toml
else
    echo "Warning: fans.toml not found in repository."
    echo "Creating a default /etc/lianlicontroller/fans.toml with example settings..."
    sudo bash -c 'cat << EOF > /etc/lianlicontroller/fans.toml
# Default Lian Li Controller configuration
color = "#FF0505"  # Red
brightness = 100.0
speed = 1000
mode = "fixed"
EOF'
fi

# Install the systemd service file
echo "Installing systemd service file to /etc/systemd/system/lianlicontroller.service..."
sudo cp lianlicontroller.service /etc/systemd/system/lianlicontroller.service

# Reload systemd to recognize the new service
echo "Reloading systemd daemon..."
sudo systemctl daemon-reload

# Enable the service to start on boot
echo "Enabling lianlicontroller service..."
sudo systemctl enable lianlicontroller

# Start the service immediately
echo "Starting lianlicontroller service..."
sudo systemctl start lianlicontroller

echo "Installation complete!"
echo "Service status can be checked with: sudo systemctl status lianlicontroller"
echo "Logs can be viewed with: journalctl -u lianlicontroller"
echo "Config file is at /etc/lianlicontroller/fans.toml. Edit it and restart the service if needed:"
echo "  sudo systemctl restart lianlicontroller"