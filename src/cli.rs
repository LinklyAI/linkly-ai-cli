use clap::{Args, Parser, Subcommand};

/// Where to send users who need more than `--help` can give them. Kept pointing
/// at the docs community page rather than individual platforms so that adding
/// or swapping a channel never requires shipping a new CLI build.
pub const COMMUNITY_URL: &str = "https://linkly.ai/docs/en/community";

const AFTER_HELP: &str = "\
Docs:      https://linkly.ai/docs
Community: https://linkly.ai/docs/en/community";

#[derive(Parser)]
#[command(
    name = "linkly",
    version,
    about = "Linkly AI — search your documents and take notes from the terminal",
    after_help = AFTER_HELP
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Output in JSON format
    #[arg(long, global = true)]
    pub json: bool,

    /// Distinguish "no results" from "failed" in the exit code: 0 = results,
    /// 1 = ran fine but matched nothing, 2 = failed. Without this flag any
    /// successful run exits 0 and any failure exits 1 (the historical
    /// behaviour), so existing scripts keep working.
    #[arg(long, global = true)]
    pub exit_code: bool,
}

/// Connection parameters for commands that talk to the Linkly AI server.
#[derive(Args, Clone, Debug)]
pub struct ConnectionArgs {
    /// MCP endpoint URL (e.g. http://192.168.1.100:60606/mcp)
    #[arg(long, requires = "token")]
    pub endpoint: Option<String>,

    /// Bearer token for LAN authentication (requires --endpoint)
    #[arg(long, requires = "endpoint")]
    pub token: Option<String>,

    /// Reach your Desktop through the remote tunnel (https://mcp.linkly.ai)
    /// instead of over the local network. Requires Pro, and your Desktop must
    /// be online — the tunnel forwards to it, it is not a copy of your data in
    /// the cloud. Cloud libraries are the exception: they are served by the
    /// cloud itself, so `--library cloud://owner/slug` works even when Desktop
    /// is offline. Notes always live on the Desktop machine.
    #[arg(long, conflicts_with_all = ["endpoint", "token"])]
    pub remote: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// List all available knowledge libraries
    ListLibraries {
        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Get a bird's-eye overview of all documents or a specific library
    Explore {
        /// Restrict to a specific library. Local: a plain name or local://<id>. Cloud (--remote only): cloud://<owner>/<slug>
        #[arg(long, value_hint = clap::ValueHint::Other)]
        library: Option<String>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Locate folder paths by fuzzy keyword match (precursor to `search` when the actual path is unknown)
    FindPaths {
        /// Keywords to substring-match against file paths (comma-separated, OR semantics).
        /// Pass cross-language or spelling variants in one call, e.g. "WeChat,微信,wxid"
        #[arg(long, value_hint = clap::ValueHint::Other, value_delimiter = ',', required = true)]
        patterns: Vec<String>,

        /// Restrict to a specific library. Local: a plain name or local://<id>. Cloud (--remote only): cloud://<owner>/<slug>
        #[arg(long, value_hint = clap::ValueHint::Other)]
        library: Option<String>,

        /// Maximum number of folder candidates to return (default: 10, max: 50)
        #[arg(long)]
        limit: Option<u32>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Search indexed local documents by keywords
    Search {
        /// Search query
        query: String,

        /// Maximum number of results (default: 20, max: 50)
        #[arg(long)]
        limit: Option<usize>,

        /// Filter by document types (comma-separated, e.g. pdf,md,xlsx,csv,docx,txt)
        #[arg(long, value_delimiter = ',')]
        r#type: Option<Vec<String>>,

        /// Restrict search to a specific library. Local: a plain name or local://<id>. Cloud (--remote only): cloud://<owner>/<slug>
        #[arg(long, value_hint = clap::ValueHint::Other)]
        library: Option<String>,

        /// Glob substring-matched against the file path (no leading/trailing * needed). * = any chars incl. /, ? = one char. Examples: '*.pdf', 'papers', '/abs/dir/'
        #[arg(long)]
        path_glob: Option<String>,

        /// Inclusive lower bound on file modification time (ISO 8601 UTC). Bare date or RFC 3339, e.g. '2024-01-01' or '2024-01-01T00:00:00Z'.
        #[arg(long)]
        modified_after: Option<String>,

        /// Inclusive upper bound on file modification time (ISO 8601 UTC). Same format as --modified-after.
        #[arg(long)]
        modified_before: Option<String>,

        /// Reorder by modification time: 'newest' (most recent first), 'oldest' (earliest first), or 'default' (no reorder, equivalent to omitting the flag). The desktop schema's default is also 'default', so this keeps the three-layer contract aligned.
        #[arg(long, value_parser = ["default", "newest", "oldest"])]
        time_sort: Option<String>,

        /// Search scope: 'folder' (default) searches all indexed content; 'notes' restricts results to local markdown card notes and IGNORES --library and --path-glob. Values are validated by the desktop, not here, so a newer desktop's scopes work without upgrading the CLI
        #[arg(long, value_hint = clap::ValueHint::Other)]
        scope: Option<String>,

        /// Filter by note tags (comma-separated, AND semantics). Leading '#' is stripped and ASCII lowercased. Most useful with --scope notes
        #[arg(long, value_hint = clap::ValueHint::Other, value_delimiter = ',')]
        tags: Option<Vec<String>>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Locate specific lines within a document by regex pattern
    Grep {
        /// Regular expression pattern
        pattern: String,

        /// Document IDs to search within. Pass '-' to read the IDs from stdin, one per line
        #[arg(required = true)]
        doc_ids: Vec<String>,

        /// Lines of context before and after each match
        #[arg(short = 'C', long)]
        context: Option<usize>,

        /// Lines of context before each match
        #[arg(short = 'B', long)]
        before: Option<usize>,

        /// Lines of context after each match
        #[arg(short = 'A', long)]
        after: Option<usize>,

        /// Case-insensitive matching
        #[arg(short = 'i', long)]
        ignore_case: bool,

        /// Output mode: content, count
        #[arg(long)]
        mode: Option<String>,

        /// Maximum number of matches (default: 20, max: 100)
        #[arg(long)]
        limit: Option<usize>,

        /// Number of matches to skip for pagination
        #[arg(long)]
        offset: Option<usize>,

        /// Fuzzy whitespace matching (auto for PDF, force on/off)
        #[arg(long)]
        fuzzy_whitespace: Option<bool>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Get document outlines by IDs
    Outline {
        /// Document IDs from search. Pass '-' to read the IDs from stdin, one per line
        #[arg(required = true)]
        ids: Vec<String>,

        /// Node IDs to expand, e.g. "2,3.1" (comma-separated). Others collapse; omit to auto-fit.
        #[arg(long, value_delimiter = ',')]
        expand: Option<Vec<String>>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Read document content by ID
    Read {
        /// Document IDs from search. Pass '-' to read the IDs from stdin, one per line
        #[arg(required = true)]
        ids: Vec<String>,

        /// Starting line number (1-based)
        #[arg(long)]
        offset: Option<usize>,

        /// Number of lines to read (max: 500)
        #[arg(long)]
        limit: Option<usize>,

        /// Detail for the referenced-images block: 'none' (mapping only), 'abstract' (default: plus excerpt and word count), 'full' (plus inline OCR text)
        #[arg(long, value_parser = ["none", "abstract", "full"])]
        image_text: Option<String>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Enumerate a container without full-text matching: indexed files under a
    /// directory (--scope folder), one library's files (--scope library), or
    /// your notes (--scope notes). Notes live on the Desktop machine — with
    /// --remote this reads them through the tunnel, and there is no cloud
    /// notes store to fall back on when Desktop is offline
    List {
        /// Container scope to list. Required. 'folder' lists indexed files under a directory (omit --path to sweep all watched roots), 'library' lists one library's files (requires --library), 'notes' lists the local markdown card notes. Values are validated by the desktop, not here, so a newer desktop's scopes work without upgrading the CLI
        #[arg(long, value_hint = clap::ValueHint::Other)]
        scope: String,

        /// Which library to list (--scope library only; required there). A plain name or local://<id>; see `linkly list-libraries`
        #[arg(long, value_hint = clap::ValueHint::Other)]
        library: Option<String>,

        /// Absolute directory path to list (--scope folder, or inside a local library). An address, not a glob — if you only know a fuzzy name, run `linkly find-paths` first
        #[arg(long, value_hint = clap::ValueHint::DirPath)]
        path: Option<String>,

        /// Filter by document types (comma-separated, e.g. pdf,md,xlsx,csv,docx,txt) — --scope folder/library only
        #[arg(long, value_delimiter = ',')]
        r#type: Option<Vec<String>>,

        /// Inclusive lower bound on file modification time (ISO 8601 UTC, bare date or RFC 3339) — --scope folder/library only
        #[arg(long)]
        modified_after: Option<String>,

        /// Inclusive upper bound on file modification time (same format) — --scope folder/library only
        #[arg(long)]
        modified_before: Option<String>,

        /// Filter by tags (comma-separated, AND semantics) — --scope notes only
        #[arg(long, value_hint = clap::ValueHint::Other, value_delimiter = ',')]
        tags: Option<Vec<String>>,

        /// Maximum items to return (default: 50, max: 200). Capped at 50 while snippets are on — the notes default; folder/library default to snippets off and page up to 200 (see --snippet / --no-snippet)
        #[arg(long)]
        limit: Option<usize>,

        /// Pagination offset in sort order (default: 0)
        #[arg(long)]
        offset: Option<usize>,

        /// Sort order: 'recent' (default), 'oldest', or 'name'
        #[arg(long, value_parser = ["recent", "oldest", "name"])]
        sort: Option<String>,

        /// Attach a per-item snippet even where the scope default is off (folder/library; from the indexed abstract). Caps --limit at 50
        #[arg(long, conflicts_with = "no_snippet")]
        snippet: bool,

        /// Omit per-item snippets, allowing --limit above 50
        #[arg(long)]
        no_snippet: bool,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Create or rewrite a markdown note in your Desktop's Notes folder. With
    /// --remote the write still lands on that machine, forwarded through the
    /// tunnel — notes are never stored in the cloud, so there is no cloud
    /// library to target and none to fall back on when Desktop is offline
    NoteSave {
        /// Operation mode: 'create' writes a new note, 'edit' rewrites an existing one (edit requires --note-id and --base-version together)
        #[arg(long, value_parser = ["create", "edit"])]
        mode: String,

        /// Markdown body without YAML front matter. Pass '-' to read the body from stdin
        #[arg(long)]
        content: String,

        /// Note UUID. Required for --mode edit
        #[arg(long, value_hint = clap::ValueHint::Other)]
        note_id: Option<String>,

        /// Current version hash from `linkly list --scope notes`. Required for --mode edit
        #[arg(long, value_hint = clap::ValueHint::Other)]
        base_version: Option<String>,

        /// Note tags, comma-separated. On Desktop >= 0.11.0 this only ADDS tags (body #tokens are the source of truth; remove a tag by deleting its #token from the content) and edits may omit it. CAUTION: older Desktops treat it as the FULL replacement set on edit (tags you omit are deleted) and require it there
        #[arg(long, value_hint = clap::ValueHint::Other, value_delimiter = ',')]
        tags: Option<Vec<String>>,

        /// Display name of the application driving this call (e.g. 'Claude Code'), shown as the note's source badge in the app. Not the model name; omit if unsure. Max 64 chars (desktop-enforced)
        #[arg(long, value_hint = clap::ValueHint::Other)]
        app_name: Option<String>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Show Linkly AI app status
    Status {
        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Diagnose connection issues (use --remote or --endpoint for non-local modes)
    Doctor {
        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Run as MCP stdio bridge (for Claude Desktop, etc.). Default bridges to the local desktop; --endpoint bridges to a LAN desktop; --remote bridges through the cloud gateway (local + cloud libraries).
    Mcp {
        /// MCP endpoint URL (e.g. http://192.168.1.100:60606/mcp)
        #[arg(long, conflicts_with = "remote")]
        endpoint: Option<String>,

        /// Bridge through the cloud gateway (https://mcp.linkly.ai), reaching local and linked cloud libraries. Requires `linkly auth set-key` first.
        #[arg(long, conflicts_with = "endpoint")]
        remote: bool,
    },

    /// Print a shell completion script
    #[command(long_about = "\
Print a shell completion script to stdout.

The script is static: it completes subcommands, flags and fixed value sets. It
does not query the desktop app, so it never blocks your prompt and works with
Linkly AI closed.

  bash        linkly completions bash > /usr/local/etc/bash_completion.d/linkly
  zsh         linkly completions zsh > \"${fpath[1]}/_linkly\"
  fish        linkly completions fish > ~/.config/fish/completions/linkly.fish
  powershell  linkly completions powershell | Out-String | Invoke-Expression
  elvish      linkly completions elvish > ~/.config/elvish/lib/linkly.elv

Open a new shell afterwards. For zsh, `compinit` must already be running (it is
under oh-my-zsh); a bare zsh needs `autoload -Uz compinit && compinit` in
~/.zshrc first.")]
    Completions {
        /// Shell to generate the script for
        #[arg(value_enum)]
        shell: clap_complete::aot::Shell,
    },

    /// Update to the latest version
    SelfUpdate,

    /// Manage authentication credentials
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
}

#[derive(Subcommand)]
pub enum AuthAction {
    /// Save an API key for remote tunnel access
    SetKey {
        /// API key (format: lkai_...)
        key: String,
    },

    /// Show the stored credential and the account it belongs to
    Status,

    /// Remove the stored credential
    Logout,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// clap's own consistency checks (duplicate flags, bad defaults, broken
    /// conflicts). Cheap insurance against a malformed derive shipping.
    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("linkly").chain(args.iter().copied()))
    }

    #[test]
    fn mcp_endpoint_and_remote_are_mutually_exclusive() {
        // Bridging to a LAN desktop and bridging through the cloud gateway are
        // different upstreams; accepting both would silently pick one.
        assert!(parse(&["mcp", "--endpoint", "http://x:1/mcp", "--remote"]).is_err());
        assert!(parse(&["mcp", "--remote"]).is_ok());
        assert!(parse(&["mcp", "--endpoint", "http://x:1/mcp"]).is_ok());
        assert!(parse(&["mcp"]).is_ok());
    }

    #[test]
    fn list_requires_a_scope() {
        assert!(parse(&["list"]).is_err());
        assert!(parse(&["list", "--scope", "notes"]).is_ok());
    }

    #[test]
    fn scope_values_are_not_frozen_at_the_cli_layer() {
        // The desktop deliberately leaves `scope` unconstrained in its schema so
        // that adding a scope doesn't require every client to ship a new build
        // first (see schemas.rs on ListInput::scope). A `value_parser` enum here
        // would reintroduce exactly that: `list --scope folder` would be refused
        // locally the moment the desktop starts supporting it. Validation is the
        // server's job — it answers with a message naming the supported values.
        assert!(parse(&["list", "--scope", "folder"]).is_ok());
        assert!(parse(&["list", "--scope", "some-future-scope"]).is_ok());
        assert!(parse(&["search", "q", "--scope", "some-future-scope"]).is_ok());
    }

    #[test]
    fn note_save_requires_mode_and_content() {
        assert!(parse(&["note-save"]).is_err());
        assert!(parse(&["note-save", "--mode", "create"]).is_err());
        assert!(parse(&["note-save", "--content", "hi"]).is_err());
        assert!(parse(&["note-save", "--mode", "create", "--content", "hi"]).is_ok());
        // Only the two documented modes.
        assert!(parse(&["note-save", "--mode", "upsert", "--content", "hi"]).is_err());
    }

    #[test]
    fn search_scope_accepts_the_documented_values() {
        assert!(parse(&["search", "q", "--scope", "notes"]).is_ok());
        assert!(parse(&["search", "q", "--scope", "folder"]).is_ok());
    }

    #[test]
    fn closed_value_sets_are_still_enforced_locally() {
        // Unlike `scope`, these are bounded choices rather than a growing set —
        // a detail level, a write mode, a sort direction. The desktop is not
        // going to add a fourth `--image-text` tier, so catching a typo here
        // beats a round-trip.
        assert!(parse(&["read", "local://1", "--image-text", "inline"]).is_err());
        assert!(parse(&["note-save", "--mode", "upsert", "--content", "x"]).is_err());
        assert!(parse(&["list", "--scope", "notes", "--sort", "sideways"]).is_err());
        assert!(parse(&["search", "q", "--time-sort", "sideways"]).is_err());
    }

    #[test]
    fn list_snippet_flags_are_mutually_exclusive() {
        // Tri-state contract (list.rs): --snippet forces on, --no-snippet
        // forces off, neither defers to the server's per-scope default.
        // Passing both has no coherent meaning and must fail at parse time —
        // this locks the `conflicts_with` relation against a field rename.
        assert!(parse(&["list", "--scope", "folder", "--snippet", "--no-snippet"]).is_err());
        assert!(parse(&["list", "--scope", "folder", "--snippet"]).is_ok());
        assert!(parse(&["list", "--scope", "folder", "--no-snippet"]).is_ok());
    }

    #[test]
    fn list_comma_separated_flags_split_into_lists() {
        // `value_delimiter` is load-bearing: without it "pdf,md" would reach
        // the desktop as one bogus doc type and be rejected server-side.
        let cli = parse(&[
            "list", "--scope", "folder", "--type", "pdf,md", "--tags", "a,b",
        ])
        .expect("parse");
        let Command::List { r#type, tags, .. } = cli.command else {
            panic!("expected List, got something else");
        };
        assert_eq!(r#type.unwrap(), vec!["pdf", "md"]);
        assert_eq!(tags.unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn list_accepts_the_pr2_filter_flags() {
        // The five folder/library flags must all parse; the per-scope validity
        // matrix is the desktop's to enforce, not clap's.
        assert!(parse(&[
            "list",
            "--scope",
            "library",
            "--library",
            "local://1",
            "--path",
            "/Users/me/docs",
            "--modified-after",
            "2024-01-01",
            "--modified-before",
            "2024-12-31",
        ])
        .is_ok());
    }

    #[test]
    fn read_image_text_accepts_only_the_three_detail_levels() {
        for level in ["none", "abstract", "full"] {
            assert!(parse(&["read", "local://1", "--image-text", level]).is_ok());
        }
        assert!(parse(&["read", "local://1", "--image-text", "inline"]).is_err());
    }

    #[test]
    fn completions_accepts_every_shell_clap_complete_supports() {
        // Whatever the enum offers, the CLI must accept — a shell that parses
        // here but has no generator would be a runtime surprise.
        for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
            assert!(
                parse(&["completions", shell]).is_ok(),
                "{shell} should be accepted"
            );
        }
        assert!(parse(&["completions", "tcsh"]).is_err());
        assert!(parse(&["completions"]).is_err());
    }

    #[test]
    fn completions_needs_no_connection_arguments() {
        // It renders from the parser itself, so it must work with no desktop
        // app, no credentials and no network.
        let cli = parse(&["completions", "zsh"]).expect("parse");
        assert!(matches!(cli.command, Command::Completions { .. }));
    }

    #[test]
    fn exit_code_is_a_global_opt_in_flag() {
        // Off by default: a plain run keeps the historical 0-on-success /
        // 1-on-failure contract that existing scripts depend on.
        assert!(!parse(&["search", "q"]).expect("parse").exit_code);
        assert!(
            parse(&["search", "q", "--exit-code"])
                .expect("parse")
                .exit_code
        );
        // global = true, so it works before or after the subcommand.
        assert!(
            parse(&["--exit-code", "list", "--scope", "notes"])
                .expect("parse")
                .exit_code
        );
    }

    #[test]
    fn comma_separated_tags_split_into_a_list() {
        let cli = parse(&["search", "q", "--tags", "work,urgent"]).expect("parse");
        match cli.command {
            Command::Search { tags, .. } => {
                assert_eq!(
                    tags.as_deref(),
                    Some(&["work".to_string(), "urgent".to_string()][..])
                );
            }
            _ => panic!("expected search"),
        }
    }

    #[test]
    fn auth_gained_status_and_logout_without_losing_set_key() {
        assert!(parse(&["auth", "status"]).is_ok());
        assert!(parse(&["auth", "logout"]).is_ok());
        assert!(parse(&["auth", "set-key", "lkai_x"]).is_ok());
    }
}
