# Git Hooks Setup

This project uses Git hooks to maintain code quality and enforce commit message standards.

## Installed Hooks

### 1. Pre-commit Hook

The pre-commit hook runs automatically before each commit and performs:

- **Code Formatting Check**: Runs `cargo fmt --check` to ensure code follows Rust formatting standards
- **Linting**: Runs `cargo clippy` to catch common mistakes and enforce best practices

If any check fails, the commit is blocked until you fix the issues.

**How to fix issues:**
```bash
# Fix formatting issues
cargo fmt

# Fix clippy warnings
cargo clippy --fix --allow-dirty --allow-staged
```

### 2. Commit Message Hook

The commit-msg hook validates that commit messages follow the [Conventional Commits](https://www.conventionalcommits.org/) format.

**Format:**
```
<type>(optional scope): <description>

[optional body]

[optional footer]
```

**Valid types:**
- `feat`: A new feature
- `fix`: A bug fix
- `docs`: Documentation changes
- `style`: Code style changes (formatting, whitespace, etc.)
- `refactor`: Code refactoring (no feature or bug fix)
- `perf`: Performance improvements
- `test`: Adding or updating tests
- `build`: Build system or dependency changes
- `ci`: CI/CD configuration changes
- `chore`: Other changes (maintenance, cleanup, etc.)
- `revert`: Reverting a previous commit

**Examples:**
```bash
# Good commit messages
git commit -m "feat: add user authentication"
git commit -m "fix(download): resolve youtube cookie issue"
git commit -m "docs: update README with installation steps"
git commit -m "chore(deps): update teloxide to latest version"

# Bad commit messages (will be rejected)
git commit -m "updated stuff"
git commit -m "fix bug"
git commit -m "WIP"
```

## Bypassing Hooks (Not Recommended)

In rare cases, you may need to bypass hooks:

```bash
# Skip all hooks
git commit --no-verify -m "emergency fix"

# Or use the shorthand
git commit -n -m "emergency fix"
```

**⚠️ Warning:** Only bypass hooks when absolutely necessary (e.g., emergency hotfixes). Bypassed commits should be cleaned up later.

## Hook Installation

The hooks are already installed in `.git/hooks/`. If you need to reinstall them:

```bash
# Make sure hooks are executable
chmod +x .git/hooks/pre-commit
chmod +x .git/hooks/commit-msg
```

## Troubleshooting

### Pre-commit hook fails with "cargo not found"
Make sure Rust is installed and cargo is in your PATH:
```bash
cargo --version
```

### Pre-commit is too slow
The pre-commit hook runs `cargo clippy` on all targets. For faster commits during development, you can temporarily disable it:
```bash
git commit --no-verify
```

However, make sure to run the checks before pushing:
```bash
cargo fmt --check
cargo clippy --all-targets --all-features
```

### Commit message validation fails
Make sure your commit message:
1. Starts with a valid type (feat, fix, docs, etc.)
2. Has a colon and space after the type: `type: description`
3. Has a description of at least 10 characters
4. Follows the examples above

## CI/CD Integration

These same checks should also run in CI/CD:

```yaml
# Example GitHub Actions workflow
- name: Check formatting
  run: cargo fmt --check

- name: Run clippy
  run: cargo clippy --all-targets --all-features -- -D warnings
```

## References

- [Conventional Commits](https://www.conventionalcommits.org/)
- [Rust Style Guide](https://doc.rust-lang.org/style-guide/)
- [Clippy Lints](https://rust-lang.github.io/rust-clippy/)
