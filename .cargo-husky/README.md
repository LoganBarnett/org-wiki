# cargo-husky Pre-commit Hooks

This repository uses `cargo-husky` to automatically run `rustfmt` before each commit.

## How it works

- `cargo-husky` is configured in the root `Cargo.toml` as a dev-dependency
- When you run any cargo command (like `cargo build`, `cargo test`, etc.), cargo-husky will automatically install the git hooks
- The hooks are defined in `.cargo-husky/hooks/pre-commit`

## Installation

The hooks will be automatically installed to `.git/hooks/` the first time you run any cargo command after cloning the repository:

```bash
cargo build
# or
cargo test
```

## What the pre-commit hook does

Before each commit, the hook will:
1. Identify which Rust files (`.rs`) are staged for commit
2. Run `rustfmt` on only those staged files
3. Re-stage the formatted versions of those files
4. Proceed with the commit, ensuring all committed code is properly formatted

Note: Only files you've staged for commit will be formatted and included. Other files in your working directory remain untouched.

## Bypassing the hook (not recommended)

If you absolutely need to commit unformatted code, you can bypass the hook with:

```bash
git commit --no-verify
```

However, this is not recommended as it defeats the purpose of having the hook.
