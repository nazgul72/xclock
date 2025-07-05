# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Windows-specific system utility that enhances the system clock with additional information displayed in a custom tooltip. The project consists of two main components:

- **xclock**: Core library providing Windows API integration for clock hover detection and tooltip display
- **xclock-cli**: Command-line interface for managing the clock hook service

## Build and Development Commands

### Building the Project
```bash
# Build the entire workspace
cargo build

# Build with optimizations
cargo build --release

# Build specific component
cargo build -p xclock
cargo build -p xclock-cli
```

### Running the Application
```bash
# Run the CLI (from root directory)
cargo run -p xclock-cli -- start
cargo run -p xclock-cli -- stop
cargo run -p xclock-cli -- status
cargo run -p xclock-cli -- help

# Run from built binary
./target/release/xclock-cli start
```

### Testing and Verification
```bash
# Run all tests
cargo test

# Run tests for specific component
cargo test -p xclock
cargo test -p xclock-cli

# Check code formatting
cargo fmt --check

# Run clippy for linting
cargo clippy
```

## Architecture

### Core Components

#### xclock Library (`xclock/src/lib.rs`)
- **Windows API Integration**: Uses `winapi` crate for low-level Windows system calls
- **Global State Management**: Thread-safe statics using `AtomicPtr`, `AtomicBool`, and `OnceLock`
- **Hook System**: Implements `WH_MOUSE_LL` Windows hook for global mouse event monitoring
- **Window Detection**: Finds Windows system clock controls (`TrayClockWClass`, `ClockWClass`, etc.)
- **Native Tooltip Modification**: Modifies existing native tooltips instead of creating custom ones

#### xclock-cli Binary (`xclock-cli/src/main.rs`)
- **Command Interface**: Provides start/stop/status commands
- **Signal Handling**: Graceful shutdown with Ctrl+C via `ctrlc` crate
- **Message Loop**: Manages Windows message processing during hook operation

### Key Technical Details

#### Thread Safety
- Uses `AtomicPtr` for hook handles and window references
- `OnceLock<Mutex<T>>` for shared collections (clock windows, mouse position)
- `SafeHwnd` wrapper for safe `Send`/`Sync` of Windows handles

#### Windows API Usage
- **Hook Management**: `SetWindowsHookExW`/`UnhookWindowsHookEx` for mouse event capture
- **Window Finding**: `FindWindowW`/`FindWindowExW` for locating system clock controls
- **Tooltip Modification**: `SetWindowTextW` and `TTM_UPDATETIPTEXTW` messages for native tooltip text changes
- **Message Processing**: `PeekMessageW`/`DispatchMessageW` for Windows event handling

#### Tooltip Features
- Displays system uptime (formatted as days/hours/minutes)
- Shows Norwegian ISO week number using `chrono` crate
- Modifies native Windows tooltips in-place
- Uses both `SetWindowTextW` and tooltip control messages for compatibility
- Finds and updates `tooltips_class32` and `SysTooltip32` windows

## Development Notes

### Windows-Specific Considerations
- This is a Windows-only project using `winapi` crate
- Requires Windows API knowledge for modifications to hook behavior
- Uses unsafe code blocks for Windows API calls (properly wrapped for safety)

### Dependencies
- `winapi`: Windows API bindings with specific feature flags for required APIs
- `chrono`: Date/time handling for week number calculations
- `ctrlc`: Cross-platform signal handling for graceful shutdown

### Safety and Error Handling
- All unsafe Windows API calls are properly wrapped
- Error propagation through `Result<(), String>` for public API
- Graceful cleanup of hooks and windows on shutdown
- Defensive programming for null pointer checks

## Common Tasks

### Adding New Tooltip Information
1. Modify `generate_tooltip_text()` function in `xclock/src/lib.rs:56`
2. The text will automatically be applied to native tooltips
3. Test with different screen resolutions and taskbar positions

### Modifying Clock Detection
1. Update `find_all_clock_windows()` in `xclock/src/lib.rs:94`
2. Add new window class names to search patterns
3. Modify `is_point_in_any_clock()` for different hit detection logic

### Adding New CLI Commands
1. Extend match statement in `xclock-cli/src/main.rs:35`
2. Add corresponding help text in `print_help()` function
3. Implement new functionality in xclock library if needed