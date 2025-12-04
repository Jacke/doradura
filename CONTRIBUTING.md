# Contributing to Doradura

Thank you for your interest in contributing to Doradura! This document provides guidelines and instructions for contributing.

## Code of Conduct

Please be respectful and constructive in all interactions with the project.

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone git@github.com:YOUR_USERNAME/doradura.git`
3. Create a new branch: `git checkout -b feature/your-feature-name`
4. Make your changes
5. Test your changes: `cargo test`
6. Format your code: `cargo fmt`
7. Run linter: `cargo clippy`
8. Commit your changes: `git commit -m "Description of changes"`
9. Push to your fork: `git push origin feature/your-feature-name`
10. Open a Pull Request

## Development Setup

### Prerequisites

- Rust 1.70 or later
- yt-dlp installed and in PATH
- SQLite3
- FFmpeg (for audio/video processing)

### Environment Setup

1. Copy `.env.example` to `.env`
2. Set required environment variables:
   - `TELOXIDE_TOKEN`: Your Telegram bot token
   - `ADMIN_USERNAME`: Your Telegram username
   - Other optional settings as needed

### Building

```bash
cargo build
```

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run integration tests
cargo test --test '*'
```

## Code Style

- Follow Rust standard formatting (use `cargo fmt`)
- Follow Rust naming conventions
- Use meaningful variable and function names
- Add documentation comments (`///`) for public APIs
- Keep functions focused and reasonably sized (< 100 lines when possible)

## Commit Messages

- Use clear, descriptive commit messages
- Start with a verb in present tense (e.g., "Add", "Fix", "Update")
- Reference issues when applicable (e.g., "Fix #123")

## Pull Request Process

1. Update documentation if needed
2. Add tests for new functionality
3. Ensure all tests pass
4. Ensure code is formatted and passes clippy checks
5. Update CHANGELOG.md if applicable
6. Request review from maintainers

## Project Structure

```
doradura/
├── src/
│   ├── telegram/     # Telegram bot logic
│   ├── download/     # Download functionality
│   ├── storage/      # Database and cache
│   ├── core/         # Core utilities and config
│   ├── lib.rs        # Library entry point
│   └── main.rs       # Binary entry point
├── tests/            # Integration tests
├── docs/             # Documentation
└── examples/         # Usage examples
```

## Areas for Contribution

- Bug fixes
- New features
- Performance improvements
- Documentation improvements
- Test coverage improvements
- Code refactoring

## Reporting Bugs

When reporting bugs, please include:

- Rust version
- Operating system
- Steps to reproduce
- Expected behavior
- Actual behavior
- Error messages/logs if applicable

## Feature Requests

Feature requests are welcome! Please:

- Check if the feature already exists or is planned
- Describe the use case
- Explain the expected behavior
- Consider implementation complexity

## Questions?

- Open an issue for questions
- Check existing issues and documentation first

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
