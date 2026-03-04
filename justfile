# doradura - project automation

# Run the TUI in release mode
run:
    cargo run --package doratui --release

# Run the TUI in demo mode
demo:
    cargo run --package doratui -- --demo

# Build the whole project
build:
    cargo build --workspace

# Check for compilation errors
check:
    cargo check --workspace

# Clean build artifacts
clean:
    cargo clean
