# Linkly AI CLI

Command-line interface for [Linkly AI](https://linkly.ai) — search your local documents from the terminal.

The CLI connects to the Linkly AI desktop app's MCP server, giving you fast access to your indexed documents without leaving the terminal.

## Prerequisites

By default, the **Linkly AI desktop app** must be running with MCP server enabled. The CLI automatically discovers the app via `~/.linkly/port`. Alternatively, use LAN mode (`--endpoint` + `--token`) or Remote mode (`--remote` with a saved API key) — see [Connection Modes](#connection-modes).

## Installation

### macOS / Linux

```bash
curl -sSL https://updater.linkly.ai/cli/install.sh | sh
```

### Windows (PowerShell)

```powershell
irm https://updater.linkly.ai/cli/install.ps1 | iex
```

### Homebrew (macOS / Linux)

```bash
brew tap LinklyAI/tap
brew install linkly
```

### Cargo

```bash
cargo install linkly-ai-cli
```

### GitHub Releases

Pre-built binaries for all platforms are available on the [Releases](https://github.com/LinklyAI/linkly-ai-cli/releases) page.

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

### Find Paths

Locate folder candidates when you only know a fuzzy or cross-language container name (e.g. "WeChat" but the actual path contains `xinWeChat`). Pass several variants in `--patterns` so the keywords are OR-matched in a single call. Output is intended as a `--path-glob` source for `linkly search`.

```bash
linkly find-paths --patterns "WeChat,微信,wxid"
linkly find-paths --patterns "Notion,notion" --library my-research
linkly find-paths --patterns "Dropbox" --limit 5
```

| Option              | Description                                                                  |
| ------------------- | ---------------------------------------------------------------------------- |
| `--patterns <list>` | Comma-separated keywords (OR-matched); ASCII case-insensitive, CJK literal   |
| `--library <name>`  | Restrict to a specific library by name                                       |
| `--limit <N>`       | Maximum folder candidates returned (default: 10, max: 50)                    |

### Search Documents

```bash
linkly search "machine learning"
linkly search "API design" --limit 5
linkly search "notes" --type pdf,md,docx,pptx,epub
linkly search "attention" --library my-research
linkly search "transformer" --path-glob "*.pdf"
linkly search "quarterly report" --modified-after 2024-01-01 --modified-before 2024-12-31
linkly search "weekly notes" --time-sort newest --limit 10
```

| Option                       | Description                                                                                                                                                                                                                  |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--limit <N>`                | Maximum results (default: 20, max: 50)                                                                                                                                                                                       |
| `--type <types>`             | Filter by document types, comma-separated (e.g. `pdf,md,docx,pptx,epub,txt,html`)                                                                                                                                                      |
| `--library <name>`           | Restrict search to a specific library by name                                                                                                                                                                                |
| `--path-glob <pat>`          | Glob **substring-matched** against the file path — may appear anywhere, no leading/trailing `*` needed. `*` matches any chars (incl. `/`), `?` one char. Examples: `*.pdf`, `papers`, `/Users/me/notes/` (a full directory path scopes to that dir). When the actual path is unknown, run `linkly find-paths` first. |
| `--modified-after <iso>`     | Inclusive lower bound on file modification time. Accepts a bare date (`2024-01-01`) or RFC 3339 (`2024-01-01T00:00:00Z`). UTC. Use for explicit windows like "after January 2024".                                           |
| `--modified-before <iso>`    | Inclusive upper bound. Same format as `--modified-after`.                                                                                                                                                                    |
| `--time-sort <mode>`         | Reorder by modification time: `newest` or `oldest`. Omit (default) to keep BM25 + vector relevance ordering. Use `newest` for "recent / latest" intent without a fixed window.                                               |

### View Document Outline

Get structural outlines for one or more documents (IDs come from search results):

```bash
linkly outline <doc-id>
linkly outline <id1> <id2> <id3>
```

### Locate Lines in a Document

```bash
linkly grep "pattern" <doc-id>
linkly grep "error|warning" <doc-id> -C 3 -i
linkly grep "TODO" <doc-id> --mode count
```

| Option          | Description                                  |
| --------------- | -------------------------------------------- |
| `-C, --context` | Lines of context before and after each match |
| `-B, --before`  | Lines of context before each match           |
| `-A, --after`   | Lines of context after each match            |
| `-i`            | Case-insensitive matching                    |
| `--mode`        | Output mode: `content` or `count`            |
| `--limit`       | Maximum matches (default: 20, max: 100)      |

### Read Document Content

```bash
linkly read <doc-id>
linkly read <doc-id> --offset 50 --limit 100
```

| Option         | Description                        |
| -------------- | ---------------------------------- |
| `--offset <N>` | Starting line number (1-based)     |
| `--limit <N>`  | Number of lines to read (max: 500) |

### List Container Contents

Enumerate a container without full-text matching — indexed files under a directory, one library's files, or your notes:

```bash
linkly list --scope folder --path /Users/me/docs
linkly list --scope folder                          # all watched roots
linkly list --scope library --library "My Library"
linkly list --scope notes --tags project --json
```

| Option                           | Description                                                              |
| -------------------------------- | ------------------------------------------------------------------------ |
| `--scope <s>`                    | Required: `folder`, `library`, or `notes`                                |
| `--library <ref>`                | Which library to list (`--scope library`; a name or `local://<id>`)      |
| `--path <dir>`                   | Absolute directory to list (`--scope folder`, or inside a local library) |
| `--type <list>`                  | Filter by document types, comma-separated (folder/library)               |
| `--modified-after/-before <t>`   | Modification-time bounds, ISO 8601 UTC (folder/library)                  |
| `--tags <list>`                  | Filter notes by tags, comma-separated, AND semantics (notes)             |
| `--sort <s>`                     | `recent` (default), `oldest`, or `name`                                  |
| `--limit` / `--offset`           | Pagination (default 50, max 200; capped at 50 while snippets are on)     |
| `--snippet` / `--no-snippet`     | Force per-item snippets on/off (default: notes on, folder/library off)   |

Notes live on the Desktop machine — with `--remote` the listing reaches them through the tunnel; there is no cloud notes store.

### Save a Note

Create or edit a markdown card note in your Desktop's Notes folder:

```bash
linkly note-save --mode create --content "Remember this #idea"
echo "Piped body" | linkly note-save --mode create --content -
linkly note-save --mode edit --note-id <uuid> --base-version <hash> --content "New body #idea"
```

| Option                  | Description                                                                    |
| ----------------------- | ------------------------------------------------------------------------------ |
| `--mode <m>`            | `create` or `edit` (edit requires `--note-id` and `--base-version` together)   |
| `--content <text>`      | Markdown body without YAML front matter; `-` reads it from stdin               |
| `--note-id <uuid>`      | Which note to edit (from `linkly list --scope notes`)                          |
| `--base-version <hash>` | The version you read (compare-and-swap; from the same `list`)                  |
| `--tags <list>`         | Note tags (see the caution below before using this on edit)                    |
| `--app-name <name>`     | Hosting application's display name, shown as the note's source badge           |

Edits are compare-and-swap: a stale `--base-version` is rejected — re-read, merge, retry. Tags live in the note body as `#tokens` (the source of truth): `--tags` only **adds** tags, and you remove one by deleting its `#token` from the content. Caution: Desktops older than 0.11.0 instead require `--tags` on edit and treat it as the **full replacement set** (tags you omit are deleted). Requires Desktop >= 0.11.0.

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

### Diagnose Connection Issues

```bash
linkly doctor
linkly doctor --remote
linkly doctor --endpoint http://192.168.1.100:60606/mcp --token abc123
```

Runs a series of checks (port file, server reachability, auth, MCP round-trip) and reports pass/fail with actionable advice.

### List Libraries

List all available knowledge libraries (useful with `search --library`):

```bash
linkly list-libraries
linkly list-libraries --remote
```

### Self-Update

```bash
linkly self-update
```

## Connection Modes

The CLI supports three connection modes:

| Mode       | Flags                              | Auth                          | How it works                                    |
| ---------- | ---------------------------------- | ----------------------------- | ----------------------------------------------- |
| **Local**  | _(default)_                        | None (localhost)              | Reads `~/.linkly/port`, connects to `127.0.0.1` |
| **LAN**    | `--endpoint <url> --token <token>` | Bearer token from desktop app | Direct connection to a LAN device               |
| **Remote** | `--remote`                         | API Key via `auth set-key`    | Connects via `https://mcp.linkly.ai` tunnel     |

> **Note:** `--endpoint` and `--token` are required together for LAN access and conflict with `--remote`. For remote access, use `linkly auth set-key`. The `mcp` command also accepts `--endpoint` alone (without `--token`).

### Remote mode setup

```bash
# Save your API Key (from https://linkly.ai/dashboard)
linkly auth set-key lkai_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx

# Search via remote tunnel
linkly search "machine learning" --remote
```

### LAN mode with token

```bash
# Connect to another device on the same network (token from desktop Settings → MCP)
linkly search "report" --endpoint http://192.168.1.100:60606/mcp --token your_lan_token
```

## Options

Connection options (`--endpoint`, `--token`, `--remote`) are available on `search`, `find-paths`, `explore`, `grep`, `outline`, `read`, `list`, `note-save`, `status`, `doctor`, and `list-libraries` commands. `--endpoint` alone is also available on `mcp`. `--json` is available on all commands.

| Flag               | Scope  | Description                                                                                       |
| ------------------ | ------ | ------------------------------------------------------------------------------------------------- |
| `--endpoint <url>` | LAN    | Connect to a specific MCP endpoint (e.g. `http://192.168.1.100:60606/mcp`), requires `--token`    |
| `--token <token>`  | LAN    | Bearer token for LAN authentication (required with `--endpoint`, conflicts with `--remote`)       |
| `--remote`         | Remote | Connect via `https://mcp.linkly.ai` tunnel (conflicts with `--endpoint`, requires `auth set-key`) |
| `--json`           | Global | Output in JSON format (useful for scripting)                                                      |
| `-V, --version`    | Global | Print version                                                                                     |
| `-h, --help`       | Global | Print help                                                                                        |

## Examples

```bash
# Local search (default, requires desktop app running)
linkly search "budget report"

# Search across LAN with token
linkly search "budget report" --endpoint http://192.168.1.100:60606/mcp --token abc123

# Search via remote tunnel
linkly search "TODO" --remote

# JSON output for scripting
linkly search "TODO" --json | jq '.content'

# Pipe document content
linkly read abc123 --limit 50 | head -20
```

## Community

- [Documentation](https://linkly.ai/docs) — guides, integrations, and troubleshooting
- [Community](https://linkly.ai/docs/en/community) — every official channel in one place
- [GitHub Issues](https://github.com/LinklyAI/linkly-ai-cli/issues) — bugs and feature requests
- [YouTube](https://www.youtube.com/@LinklyAI) · [X](https://x.com/linkly_ai)

## License

Apache-2.0
