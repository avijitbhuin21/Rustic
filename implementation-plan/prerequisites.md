# Rustic — Prerequisites

Everything needed to set up the development environment and build Rustic.

---

## System Requirements

| Requirement | Minimum | Recommended |
|-------------|---------|-------------|
| **OS** | Windows 10, macOS 10.15, Ubuntu 18.04 | Windows 11, macOS 14, Ubuntu 22.04 |
| **RAM** | 8 GB | 16 GB |
| **Disk Space** | 5 GB (tools + build artifacts) | 10 GB |
| **Display** | 1280x800 | 1920x1080+ |

---

## Required Software

### 1. Rust Toolchain

Rustic's backend is written entirely in Rust. Install via rustup.

```bash
# Install rustup (Rust installer)
# Windows: Download from https://rustup.rs/ and run the installer
# macOS/Linux:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Verify installation
rustc --version    # Should be 1.75.0 or later
cargo --version
```

**Required Rust version:** 1.75.0+ (for async trait support and other modern features)

### 2. Node.js & npm

Required for the frontend dev server (Vite) and xterm.js dependency.

```bash
# Install Node.js LTS (v20+)
# Windows: Download from https://nodejs.org/
# macOS: brew install node
# Linux: https://nodejs.org/en/download/package-manager

# Verify
node --version     # Should be v20.0.0 or later
npm --version      # Should be 10.0.0 or later
```

### 3. Tauri 2 CLI

```bash
# Install Tauri CLI
cargo install tauri-cli --version "^2"

# Verify
cargo tauri --version
```

### 4. Vite

Installed as a project dev dependency (not global), but listed here for awareness.

```bash
# Will be installed via npm install in the project
# No global install needed
```

### 5. Platform-Specific Dependencies

#### Windows
- **Microsoft Visual Studio C++ Build Tools** — Required by Rust
  - Install via Visual Studio Installer → "Desktop development with C++"
  - Or: `winget install Microsoft.VisualStudio.2022.BuildTools`
- **WebView2** — Ships with Windows 10 (1803+) and Windows 11. If missing:
  - Download from https://developer.microsoft.com/en-us/microsoft-edge/webview2/

#### macOS
- **Xcode Command Line Tools**:
  ```bash
  xcode-select --install
  ```

#### Linux (Debian/Ubuntu)
```bash
sudo apt update
sudo apt install -y \
  build-essential \
  libwebkit2gtk-4.1-dev \
  libssl-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  wget \
  file \
  curl
```

#### Linux (Fedora)
```bash
sudo dnf install -y \
  webkit2gtk4.1-devel \
  openssl-devel \
  gtk3-devel \
  libappindicator-gtk3-devel \
  librsvg2-devel
```

#### Linux (Arch)
```bash
sudo pacman -S --needed \
  webkit2gtk-4.1 \
  base-devel \
  openssl \
  gtk3 \
  libappindicator-gtk3 \
  librsvg
```

---

## Optional Software (Recommended)

### 6. Git

Required for source control features and project management.

```bash
# Windows: Download from https://git-scm.com/ or winget install Git.Git
# macOS: brew install git (or comes with Xcode CLI tools)
# Linux: sudo apt install git

git --version     # Should be 2.30.0 or later
```

### 7. FFmpeg

For converting the SVG logo to favicon and other icon formats.

```bash
# Windows: Already installed (per user)
# macOS: brew install ffmpeg
# Linux: sudo apt install ffmpeg

ffmpeg -version
```

### 8. Language Servers (for LSP — Phase 13)

These are not needed until Phase 13, but listed for future reference.

| Language | Server | Install |
|----------|--------|---------|
| Rust | rust-analyzer | `rustup component add rust-analyzer` |
| JavaScript/TypeScript | typescript-language-server | `npm install -g typescript-language-server typescript` |
| Python | pyright | `npm install -g pyright` |
| Go | gopls | `go install golang.org/x/tools/gopls@latest` |
| C/C++ | clangd | Package manager (apt/brew/pacman) |
| HTML/CSS | vscode-langservers-extracted | `npm install -g vscode-langservers-extracted` |
| JSON | vscode-langservers-extracted | (same as above) |

---

## Rust Crate Dependencies

These are installed automatically via `cargo build`, listed here for awareness and version pinning.

### rustic-core
| Crate | Version | Purpose |
|-------|---------|---------|
| `ropey` | ~1.6 | Rope data structure for text buffers |
| `tree-sitter` | ~0.24 | Incremental parsing framework |
| `tree-sitter-rust` | ~0.23 | Rust grammar |
| `tree-sitter-javascript` | ~0.23 | JavaScript grammar |
| `tree-sitter-typescript` | ~0.23 | TypeScript grammar |
| `tree-sitter-python` | ~0.23 | Python grammar |
| `tree-sitter-go` | ~0.23 | Go grammar |
| `tree-sitter-c` | ~0.23 | C grammar |
| `tree-sitter-cpp` | ~0.23 | C++ grammar |
| `tree-sitter-java` | ~0.23 | Java grammar |
| `tree-sitter-json` | ~0.24 | JSON grammar |
| `tree-sitter-toml` | ~0.6 | TOML grammar |
| `tree-sitter-html` | ~0.23 | HTML grammar |
| `tree-sitter-css` | ~0.23 | CSS grammar |
| `tree-sitter-md` | ~0.4 | Markdown grammar |
| `serde` | ~1.0 | Serialization/deserialization |
| `serde_json` | ~1.0 | JSON support |
| `ignore` | ~0.4 | .gitignore-aware file walking (from ripgrep) |
| `grep-regex` | ~0.1 | Regex search (from ripgrep) |
| `grep-searcher` | ~0.1 | Content search (from ripgrep) |
| `uuid` | ~1.0 | Unique identifiers |

### rustic-db
| Crate | Version | Purpose |
|-------|---------|---------|
| `rusqlite` | ~0.32 | SQLite bindings (with `bundled` feature) |
| `serde` | ~1.0 | Serialization |
| `serde_json` | ~1.0 | JSON for flexible columns |

### rustic-agent
| Crate | Version | Purpose |
|-------|---------|---------|
| `reqwest` | ~0.12 | HTTP client for AI API calls |
| `keyring` | ~3.0 | OS keychain for API key storage |
| `tokio` | ~1.0 | Async runtime |
| `serde` | ~1.0 | Serialization |
| `serde_json` | ~1.0 | JSON for API payloads |
| `futures` | ~0.3 | Stream abstractions for SSE |
| `async-trait` | ~0.1 | Async trait support |

### rustic-git
| Crate | Version | Purpose |
|-------|---------|---------|
| `git2` | ~0.19 | libgit2 bindings for git operations |
| `serde` | ~1.0 | Serialization |

### rustic-terminal
| Crate | Version | Purpose |
|-------|---------|---------|
| `portable-pty` | ~0.8 | Cross-platform PTY spawning |
| `tokio` | ~1.0 | Async I/O for PTY read/write |
| `bytes` | ~1.0 | Byte buffer handling |

### src-tauri
| Crate | Version | Purpose |
|-------|---------|---------|
| `tauri` | ~2.0 | Application framework |
| `tauri-plugin-dialog` | ~2.0 | Native file/folder dialogs |
| `tauri-plugin-shell` | ~2.0 | Shell command execution |
| `tauri-plugin-fs` | ~2.0 | File system access |
| `serde` | ~1.0 | Serialization |
| `serde_json` | ~1.0 | JSON |
| `tokio` | ~1.0 | Async runtime |

### Frontend (npm)
| Package | Version | Purpose |
|---------|---------|---------|
| `xterm` | ~5.3 | Terminal emulation in browser |
| `@xterm/addon-fit` | ~0.10 | Auto-resize terminal to container |
| `@tauri-apps/api` | ~2.0 | Tauri IPC (invoke, events, window) |
| `vite` | ~6.0 | Dev server for Tauri |

---

## Environment Verification

Run this checklist before starting development:

```bash
# 1. Rust toolchain
rustc --version          # >= 1.75.0
cargo --version

# 2. Node.js
node --version           # >= 20.0.0
npm --version            # >= 10.0.0

# 3. Tauri CLI
cargo tauri --version    # >= 2.0.0

# 4. Git
git --version            # >= 2.30.0

# 5. Platform-specific (Windows)
# Check WebView2: open Edge, navigate to edge://version — WebView2 Runtime should be listed
# Check MSVC: run `cl` in Developer Command Prompt

# 6. Platform-specific (Linux)
pkg-config --libs webkit2gtk-4.1   # Should output library flags

# 7. FFmpeg (optional)
ffmpeg -version
```

---

## Initial Project Setup

Once prerequisites are verified, the project is initialized in Phase 1, Step 1.1 of the implementation plan. The basic steps are:

```bash
# 1. Navigate to project directory
cd d:/Programming/Projects/Personal/Rustic

# 2. Initialize npm project and install frontend deps
npm init -y
npm install @tauri-apps/api xterm @xterm/addon-fit
npm install -D vite

# 3. Initialize Cargo workspace (Cargo.toml at root)
# 4. Create src-tauri/ with Tauri app
# 5. Create crate directories under crates/
# 6. Verify build
cargo build
npm run tauri dev
```

Detailed setup instructions are in [PLAN.md](PLAN.md) — Phase 1, Step 1.1.
