# SM83 Gameboy Emulator

[![Rust](https://github.com/ThaumielSparrow/rust-sm83/actions/workflows/rust.yml/badge.svg)](https://github.com/ThaumielSparrow/rust-sm83/actions)


Gameboy/Gameboy Color emulator. Core functionality based on implementation from Mathijs "mvdnes" van de Nes (see LICENSE file).

Supports Windows, MacOS, and Linux (tested on Ubuntu LTS 22.04/24.04). Prebuilt binary is included for Windows.

### Usage: 

Build with `cargo b --release`.

On Linux systems, some system dependencies are required:

```sudo apt-get update && sudo apt-get install -y libasound2-dev libudev-dev libgtk-3-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libwayland-dev libxkbcommon-dev mesa-common-dev```

Run with `cargo r --release`, or run the executable in `target\release`. Select your ROM using the GUI, drag it onto the window, or pass its path as the first argument.

Supports saving in-game (battery-backed RAM) or with savestates. Gamepads are supported (bindings under Settings > Keybindings), as are GameShark RAM cheats (Emulation > Cheats).

Files written next to the ROM: `<rom>.gbsave` (battery RAM, interchangeable with other emulators), `<rom>.state` (session auto-save), and `<rom>.slotN.state` (save-state slots). `config.json` and `crash.log` are written next to the executable.

### Emulator Keybinds:

Emulator keybinds are configurable and can be adjusted from Settings > Keybindings while the emulator is running.

`A`: `X`

`B`: `Z`

`Dpad`: `ArrowKeys`

`Start`: `Enter`

`Select`: `Space`

### System Keybinds:

These are reserved keybinds for system functionality and cannot be adjusted through config menu.

`F1-F4`: Save state, slots 1-4

`F5-F8`: Load state, slots 1-4

`F9`: Toggle FPS overlay

`F11`: Toggle fullscreen

`F12`: Screenshot (PNG saved next to the ROM)

`T`: Toggle turbo

`Shift`: Hold turbo

`Y`: Toggle interpolation

`P`: Pause

`M`: Mute

`Backspace`: Hold to rewind

`\`: Step back one rewind snapshot

`Ctrl+R`: Reset

`Esc`: Close menu/Close emulator (double-press required to close emulator)

Turbo speed is configurable via the config menu.
