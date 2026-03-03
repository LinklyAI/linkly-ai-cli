# Linkly AI CLI

Command-line interface for [Linkly AI](https://linkly.ai) — search your local documents from the terminal.

The CLI connects to the Linkly AI desktop app's MCP server, giving you fast access to your indexed documents without leaving the terminal.

## Prerequisites

The **Linkly AI desktop app** must be running with MCP server enabled. The CLI automatically discovers the app via `~/.linkly/port`.

## Installation

### macOS / Linux

```bash
curl -sSL https://updater.linkly.ai/cli/install.sh | sh
```

### Windows (PowerShell)

```powershell
irm https://updater.linkly.ai/cli/install.ps1 | iex
```

### GitHub Releases

Pre-built binaries for all platforms are available on the [Releases](https://github.com/nicepkg/linkly-ai-cli/releases) page.

| Platform              | File                                      |
| --------------------- | ----------------------------------------- |
| macOS (Apple Silicon) | `linkly-aarch64-apple-darwin.tar.gz`      |
| macOS (Intel)         | `linkly-x86_64-apple-darwin.tar.gz`       |
| Linux (x86_64)        | `linkly-x86_64-unknown-linux-gnu.tar.gz`  |
| Linux (ARM64)         | `linkly-aarch64-unknown-linux-gnu.tar.gz` |
| Windows (x64)         | `linkly-x86_64-pc-windows-msvc.zip`       |

### From Source

```bash
cargo install --path .
```

## Usage

### Search Documents

```bash
linkly search "machine learning"
linkly search "API design" --limit 5
linkly search "notes" --type pdf,md,docx
```

| Option           | Description                                                             |
| ---------------- | ----------------------------------------------------------------------- |
| `--limit <N>`    | Maximum results (default: 20, max: 50)                                  |
| `--type <types>` | Filter by document types, comma-separated (e.g. `pdf,md,docx,txt,html`) |

### View Document Outline

Get structural outlines for one or more documents (IDs come from search results):

```bash
linkly outline <doc-id>
linkly outline <id1> <id2> <id3>
```

### Read Document Content

```bash
linkly read <doc-id>
linkly read <doc-id> --offset 50 --limit 100
```

| Option         | Description                        |
| -------------- | ---------------------------------- |
| `--offset <N>` | Starting line number (1-based)     |
| `--limit <N>`  | Number of lines to read (max: 500) |

### Check Status

```bash
linkly status
```

### MCP Bridge Mode

Run as a stdio MCP server for Claude Desktop, Cursor, or other MCP clients:

```bash
linkly mcp
```

Claude Desktop configuration (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "linkly-ai": {
      "command": "linkly",
      "args": ["mcp"]
    }
  }
}
```

### Self-Update

```bash
linkly self-update
```

## Global Options

| Flag               | Description                                                                |
| ------------------ | -------------------------------------------------------------------------- |
| `--endpoint <url>` | Connect to a specific MCP endpoint (e.g. `http://192.168.1.100:60606/mcp`) |
| `--json`           | Output in JSON format (useful for scripting)                               |
| `-v, --verbose`    | Verbose output                                                             |
| `-V, --version`    | Print version                                                              |
| `-h, --help`       | Print help                                                                 |

## Examples

```bash
# Search across LAN
linkly search "budget report" --endpoint http://192.168.1.100:60606/mcp

# JSON output for scripting
linkly search "TODO" --json | jq '.content'

# Pipe document content
linkly read abc123 --limit 50 | head -20
```

## License

Apache-2.0
