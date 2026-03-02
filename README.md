# Linkly AI CLI

Command-line interface for [Linkly AI](https://linkly.ai) — search your local documents from the terminal.

The CLI connects to the Linkly AI desktop app's MCP server, giving you fast access to your indexed documents without leaving the terminal.

## Installation

### From Binary (Recommended)

Download the latest release for your platform:

```bash
# macOS (Apple Silicon)
curl -L https://updater.linkly.ai/cli/latest/darwin-aarch64.tar.gz | tar xz
sudo mv linkly /usr/local/bin/

# macOS (Intel)
curl -L https://updater.linkly.ai/cli/latest/darwin-x86_64.tar.gz | tar xz
sudo mv linkly /usr/local/bin/

# Linux (x86_64)
curl -L https://updater.linkly.ai/cli/latest/linux-x86_64.tar.gz | tar xz
sudo mv linkly /usr/local/bin/
```

### From Source

```bash
cargo install --path .
```

## Prerequisites

The Linkly AI desktop app must be running with MCP server enabled. The CLI automatically discovers the app via `~/.linkly/port`.

## Usage

### Search Documents

```bash
linkly search "machine learning"
linkly search "API design" --limit 5
linkly search "notes" --type md,txt
```

### View Document Outline

```bash
linkly outline <doc-id>
linkly outline <id1> <id2> <id3>
```

### Read Document Content

```bash
linkly read <doc-id>
linkly read <doc-id> --offset 50 --limit 100
```

### Check Status

```bash
linkly status
```

### MCP Bridge Mode

Use as a stdio MCP server for Claude Desktop or other MCP clients:

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

| Flag | Description |
|------|-------------|
| `--endpoint <url>` | Connect to a specific MCP endpoint (e.g. LAN access) |
| `--json` | Output in JSON format |
| `-v, --verbose` | Verbose output |

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

MIT
