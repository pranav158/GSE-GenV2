# 🎮 GSE-Gen v2 — Goldberg Steam Emulator Settings Generator

A fast, automated **command-line tool** that generates a complete `steam_settings` directory for any Steam game, ready to use with the [Goldberg Steam Emulator (GBE)](https://github.com/Detanup01/gbe_fork).

## ✨ Features

- **App ID or Name** — Pass a numeric App ID or search by game name
- **DLC Enumeration** — Automatically fetches all DLC IDs and names
- **Achievements** — Downloads achievement schema and icon images
- **Branches & Depots** — Retrieves branch info and depot IDs from package data
- **Language Detection** — Parses supported languages from Steam store data
- **Interface Extraction** — Optionally extracts `steam_interfaces.txt` from `steam_api64.dll`
- **Config Generation** — Produces `configs.app.ini`, `configs.user.ini`, `configs.main.ini`, `configs.overlay.ini`
- **Beautiful CLI** — Progress bars, spinners, and colored output

## 📋 Prerequisites

- [Rust](https://rustup.rs/) (1.70+ recommended)
- A **Steam Web API Key** — get one free at [steamcommunity.com/dev/apikey](https://steamcommunity.com/dev/apikey)

## 🚀 Installation

```bash
git clone https://github.com/pranav158/GSE-GenV2.git
cd GSE-GenV2
cargo build --release
```

The binary will be at `target/release/gse-gen.exe` (Windows) or `target/release/gse-gen` (Linux/macOS).

## 🔑 API Key Configuration

The tool requires a Steam Web API key. You can provide it in two ways:

### Option 1: Environment Variable (Recommended)

```bash
# Linux / macOS
export STEAM_API_KEY="YOUR_KEY_HERE"

# Windows (PowerShell)
$env:STEAM_API_KEY = "YOUR_KEY_HERE"

# Windows (CMD)
set STEAM_API_KEY=YOUR_KEY_HERE
```

Or create a `.env` file in the project root:

```env
STEAM_API_KEY=YOUR_KEY_HERE
```

### Option 2: CLI Flag

```bash
gse-gen 3764200 --api-key YOUR_KEY_HERE
```

## 📖 Usage

```bash
# By App ID
gse-gen 3764200

# By game name
gse-gen "Resident_Evil_Requiem"

# Custom output directory
gse-gen 3764200 --output ./my_settings

# Custom username and SteamID
gse-gen 3764200 --username "MyName" --steamid "76561198999999999"

# Unlock all DLC
gse-gen 3764200 --unlock-all-dlc

# Skip achievement images (faster)
gse-gen 3764200 --no-images

# Skip achievements entirely
gse-gen 3764200 --no-achievements

# Extract steam_interfaces.txt from a DLL
gse-gen 3764200 --steam-api "path/to/steam_api64.dll"

# Combine flags
gse-gen "Resident_Evil_Requiem" --unlock-all-dlc --no-images --username "Player1"
```

## 📁 Output Structure

```
GameName (AppID)/
└── steam_settings/
    ├── steam_appid.txt
    ├── supported_languages.txt
    ├── configs.app.ini          # DLC list + unlock_all setting
    ├── configs.user.ini         # Username, SteamID, language
    ├── configs.main.ini         # Connectivity settings
    ├── configs.overlay.ini      # Overlay appearance
    ├── branches.json            # Branch/build info
    ├── depots.txt               # Depot IDs
    ├── achievements.json        # Achievement definitions
    ├── steam_interfaces.txt     # (optional) Extracted interfaces
    ├── achievement_images/      # Achievement icons
    ├── fonts/
    └── sounds/
```

> **Note:** After generation, make sure to update `configs.user.ini` inside the `steam_settings` folder with your own **username**, **SteamID64**, and preferred **language**. You can also set these at generation time using `--username` and `--steamid` flags.

## ⚙️ CLI Reference

| Argument | Description | Default |
|---|---|---|
| `<QUERY>` | Steam App ID or game name | *(required)* |
| `-o, --output <DIR>` | Output directory | `<GameName> (<AppID>)` |
| `--username <NAME>` | Username for `configs.user.ini` | `Player` |
| `--steamid <ID>` | SteamID64 for `configs.user.ini` | `76561198999999999` |
| `--api-key <KEY>` | Steam Web API key (or set `STEAM_API_KEY` env var) | *(required)* |
| `--no-images` | Skip downloading achievement icons | `false` |
| `--no-achievements` | Skip achievements entirely | `false` |
| `--unlock-all-dlc` | Set `unlock_all=1` in DLC config | `false` |
| `--steam-api <PATH>` | Path to `steam_api64.dll` for interface extraction | *(none)* |

## 🛠️ Building from Source

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run directly
cargo run -- 3764200
```

## 📄 License

This project is provided as-is for educational and personal use.
