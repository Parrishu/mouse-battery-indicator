# Mouse Battery Indicator

Shows a battery icon for a compatible Logitech mouse in the Windows system tray.

![Screenshot of indicator on Windows task bar](docs/icon-screenshot.png)

## Features

* Supports Windows 10 and newer
* Supports Logitech HID++ devices that expose battery information, including many LIGHTSPEED, Unifying, and Bolt mice
* Works with a compatible mouse connected by its receiver or USB cable
* Detects the device's firmware-reported name instead of requiring a specific mouse model
* Built with Rust and uses very little memory
* Shows a charging indicator

  ![Charging icon](docs/icon-charging.png)

* Can display the battery percentage instead of an icon

  ![Percentage icon](docs/icon-text.png)

* Optional low-battery, critical-battery, charging-started, and battery-full notifications
* Portable mode stores settings beside the executable in `mouse-battery-indicator-settings.toml`

  Default settings:
  ```toml
  NotificationsEnabled = true
  UseNumberIcon = false
  ```

Mouse Battery Indicator uses HIDAPI and Logitech's HID++ protocol. It reads Unified Battery (`0x1004`), Battery Status (`0x1000`), Battery Voltage (`0x1001`), and ADC Measurement (`0x1F20`) when exposed by the device. Only one battery-capable Logitech device is shown at a time; the first detected device is used.

## Installation

Download the latest release and run the MouseBatteryIndicatorSetup installer.

Running the installer may trigger a Microsoft Defender SmartScreen warning. Windows may also falsely flag a new unsigned or infrequently downloaded executable as a virus. This repository's GitHub Actions workflow builds the releases from source and can attest the build output.

## Portable mode

Run `mouse-battery-indicator-portable.exe` to use the application without installing it. Portable mode does not use the Windows registry:

* Settings: `mouse-battery-indicator-settings.toml`
* Log: `mouse-battery-indicator.log`

## Troubleshooting

Run `mouse-battery-indicator-debug.exe` for console logging, or open `mouse-battery-indicator.log` beside the executable.

### The mouse is not detected

Confirm that:

1. The mouse is powered on, awake, and paired with its Logitech receiver, or connected by USB.
2. The receiver is connected directly to the computer.
3. The mouse exposes one of the HID++ battery features listed above; support varies by model and firmware.
4. Run `mouse-battery-indicator-debug.exe` and check its console output for HID++ probe messages.

If the mouse is still not detected, include the debug log when reporting the issue.

### Battery values are coarse

The amount of battery detail depends on what the mouse and HID protocol expose. Some devices report coarse levels instead of exact percentages.

## Development

Git, Rust, and Cargo are required. This project also depends on HIDAPI, stored as a Git submodule.

On Windows, Visual Studio 2019 or later is required with:

* Desktop development with C++
* Windows SDK
* MSVC C++ x64/x86 build tools

Clone the repository and initialize its submodules:

```sh
git clone https://github.com/Parrishu/mouse-battery-indicator --recursive
cd mouse-battery-indicator
```

Build and run:

```sh
cargo run --release
cargo run
```

Build the portable executable:

```sh
cargo build --release --bin mouse-battery-indicator-portable --features portable
```

Build the Inno Setup installer by opening [installer.iss](installer.iss) in Inno Setup Compiler.

## Translations

Translations are maintained in [src/lang.rs](./src/lang.rs).

## License

This project is licensed under GNU GPL v3. See [LICENSE](./LICENSE) for the full terms.
