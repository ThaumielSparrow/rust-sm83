# SM83 Gameboy Emulator

[![Rust](https://github.com/ThaumielSparrow/rust-sm83/actions/workflows/rust.yml/badge.svg)](https://github.com/ThaumielSparrow/rust-sm83/actions)


Gameboy/Gameboy Color emulator. Core functionality based on implementation from Mathijs "mvdnes" van de Nes (see LICENSE file).

Supports Windows, MacOS, and Linux (tested on Ubuntu LTS 22.04/24.04). Prebuilt binary is included for Windows.

### Usage: 

Build with `cargo b --release`.

On Linux systems, some system dependencies are required:

```sudo apt-get update && sudo apt-get install -y libasound2-dev libgtk-3-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libwayland-dev libxkbcommon-dev mesa-common-dev```

Run with `cargo r --release`, or run the executable in `target\release`. Select your ROM using the GUI.

Supports saving in-game (battery-backed RAM) or with savestates.

### Emulator Keybinds:

Emulator keybinds are configurable and can be adjusted from the Options dropdown while emulator is running.

`A`: `Z`

`B`: `X`

`Dpad`: `ArrowKeys`

`Start`: `Space`

`Select`: `Enter`

### System Keybinds:

These are reserved keybinds for system functionality and cannot be adjusted through config menu.

`F1-F4`: Save state, slots 1-4

`F5-F8`: Load state, slots 1-4

`T`: Toggle turbo

`RShift`: Hold turbo

`Y`: Toggle interpolation

`Esc`: Close menu/Close emulator (double-press required to close emulator)

Turbo speed is configurable via the config menu.
