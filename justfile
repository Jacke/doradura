# doradura - project automation

set dotenv-filename := x'${ENV_FILE:-.env}'

# Run the TUI in release mode
run:
    cargo run --package doratui --release

# Run the TUI in demo mode
demo:
    cargo run --package doratui -- --demo

# Run the bot locally
run-bot:
    cargo run --package doradura

# Run the bot with staging config (.env.staging)
run-stage:
    cargo run --package doradura -- run-staging

# Build the whole project
build:
    cargo build --workspace

# Check for compilation errors
check:
    cargo check --workspace

# Run all tests
test:
    cargo test --workspace

# Run clippy lints
clippy:
    cargo clippy --workspace -- -D warnings

# Format code
fmt:
    cargo fmt --all

# Full lint pass (fmt + clippy + test)
lint: fmt clippy test

# Clean build artifacts
clean:
    cargo clean
