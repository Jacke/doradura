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

# Clean build artifacts
clean:
    cargo clean
