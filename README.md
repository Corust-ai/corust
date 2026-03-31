# corust-cli

A terminal UI client for the [Corust](https://corust.ai) agent, built on the [Agent Client Protocol (ACP)](https://github.com/anthropics/agent-client-protocol).

## Features

- Full TUI experience powered by [Ratatui](https://ratatui.rs)
- Markdown rendering with syntax highlighting
- Streaming agent responses

## Installation

### From source

```bash
cargo install --path cli
```

### Prerequisites

- Rust 2024 edition (1.85+)

## Usage

```bash
corust-cli
```

Options:

| Flag | Description |
|------|-------------|
| `--cwd <DIR>` | Set the working directory for the session |

## Development

```bash
# Build
cargo build

# Run
cargo run -p corust-cli
```

## License

[MIT](LICENSE)
