<p align="center">
  <img src="assets/promplet-icon.png" width="112" alt="Promplet icon">
</p>

# Promplet

Promplet is a tiny, always-available prompt strip inspired by the Classic Mac
OS Control Strip. Click a button and its saved text is inserted into the
previously focused text box without reading or changing the clipboard.

![Promplet floating above the Windows taskbar](docs/promplet-windows.png)

## Windows alpha

The `v0.1.0-alpha.1` checkpoint is a portable Windows x86-64 app: extract the
ZIP and run `promplet.exe`. There is no installer, background service,
auto-updater, or telemetry.

Windows may show a security warning because this early build is not
code-signed.

## Use

- Click a prompt to insert its text.
- Right-click a prompt to edit, duplicate, add after, or delete it.
- Drag the dotted grip to move the strip.
- Right-click the grip to add a prompt, reveal the config file, or quit.

Promplet starts just above the taskbar. You may drag it over the taskbar, and
its topmost behavior yields while another application is truly full-screen.

Settings are stored as readable JSON at:

```text
%LOCALAPPDATA%\Promplet\promplets.json
```

Use **Show config** from the grip menu to open that folder for backup or
transfer.

## Build from source

Prerequisites:

- Rust stable with the `x86_64-pc-windows-msvc` target
- Visual Studio 2022 Build Tools with the C++ workload and Windows SDK

```powershell
cargo test
cargo run --release
```

Debug builds keep a console window for diagnostics; release builds use the
Windows GUI subsystem.

## Current scope

Windows is implemented first. Native macOS and Linux input/window backends are
planned, but not present in this alpha.

- Windows prevents input injection into an elevated app unless Promplet is
  elevated too.
- Some applications and secure text fields intentionally reject synthetic
  keyboard input.

## License

[MIT](LICENSE)
