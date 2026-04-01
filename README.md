# corust-cli

A terminal UI client for the [Corust](https://corust.ai) agent, built on the [Agent Client Protocol (ACP)](https://github.com/anthropics/agent-client-protocol).

## Features

- Full TUI experience powered by [Ratatui](https://ratatui.rs)
- Markdown rendering with syntax highlighting
- Streaming agent responses

## Installation

### Quick install (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/Corust-ai/corust-cli/main/install.sh | bash
```

Or install a specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/Corust-ai/corust-cli/main/install.sh | bash -s v0.1.0
```

### Download from releases

Pre-built binaries for macOS and Linux are available on the [Releases](https://github.com/Corust-ai/corust-cli/releases) page.

### From source

Requires Rust 2024 edition (1.85+).

```bash
cargo install --path cli
```

Note: building from source only installs `corust-cli`. You also need the `corust-agent-acp` binary — place it in the same directory as `corust-cli`, or set `CORUST_ACP_BIN` to its path.

## Usage

```bash
corust-cli
```

Options:

| Flag | Description |
|------|-------------|
| `-C, --project-dir <DIR>` | Set the working directory for the session |
| `--server-bin <PATH>` | Path to the corust-agent-acp binary |
| `-e, --exec <PROMPT>` | Non-interactive mode: execute a single prompt and exit |
| `-t, --tui` | Launch the TUI instead of the line-based REPL |

## Development

```bash
# Build
cargo build

# Run
cargo run -p corust-cli
```

## License

[MIT](LICENSE)
