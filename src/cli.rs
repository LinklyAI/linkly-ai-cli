use clap::{Args, Parser, Subcommand};

/// Where to send users who need more than `--help` can give them. Kept pointing
/// at the docs community page rather than individual platforms so that adding
/// or swapping a channel never requires shipping a new CLI build.
pub const COMMUNITY_URL: &str = "https://linkly.ai/docs/en/community";

const AFTER_HELP: &str = "\
Connections (commands that read or write documents):
  (no flag)   the Linkly AI Desktop on this machine
  --lan       a Desktop on your local network, saved with
              `linkly auth set-key <token> --lan --endpoint <url>`
  --remote    the remote gateway (https://mcp.linkly.ai): your Desktop through
              the tunnel (Pro, Desktop online) plus linked cloud:// libraries
              (any plan, Desktop may be offline). Sign in: `linkly auth login`
              (agents: see `linkly auth login --help`)
  `linkly doctor` checks all of them; notes always live on the Desktop machine.

Environment:
  LINKLY_CONFIG_DIR      where credentials live (default ~/.linkly); one directory = one account
  LINKLY_NO_SKILLS_HINT  set to 1 to silence the agent-skill update notice
  LINKLY_WEB_URL         override https://linkly.ai for `auth login` (development only)

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

    /// Print one JSON object per result (errors too), for programs that parse
    /// the output. If you read the output yourself, leave it off: plain text is
    /// shorter. Has no effect on set-key, self-update, skills install/update,
    /// completions and mcp
    #[arg(long, global = true)]
    pub json: bool,

    /// Exit 0 = results, 1 = nothing matched (search, find-paths, grep, list
    /// only), 2 = failed. Without it: 0 = success, 1 = failure. Invalid
    /// arguments always exit 2
    #[arg(long, global = true)]
    pub exit_code: bool,

    /// Name of the calling AI application, e.g. `claude-code`. Agents: always pass it.
    ///
    /// Desktop keeps a local access log so the user can see which applications
    /// read their documents; without a name your calls land there as an
    /// anonymous "local" entry and the user cannot tell you apart from anything
    /// else on the machine.
    ///
    /// It is a label, never a credential: it grants nothing and is never
    /// checked against anything. Printable ASCII, up to 64 characters; anything
    /// else is ignored rather than rejected, so a bad value never fails a call.
    /// `linkly mcp` names the connecting application by itself; this overrides it.
    #[arg(long, global = true, value_name = "NAME", value_hint = clap::ValueHint::Other)]
    pub client: Option<String>,
}

/// Connection parameters for commands that talk to the Linkly AI server.
#[derive(Args, Clone, Debug)]
pub struct ConnectionArgs {
    /// Use the remote gateway instead of this machine's Desktop (see "Connections" in `linkly --help`)
    #[arg(long, conflicts_with_all = ["endpoint", "token"])]
    pub remote: bool,

    /// Use the LAN Desktop saved by `linkly auth set-key <token> --lan --endpoint <url>`
    #[arg(long, conflicts_with_all = ["endpoint", "token", "remote"])]
    pub lan: bool,

    /// One-off LAN Desktop address, not saved (with or without /mcp); requires --token. Prefer --lan
    #[arg(long, requires = "token")]
    pub endpoint: Option<String>,

    /// LAN access token for --endpoint (from Desktop > Agent usage > View MCP connection settings)
    #[arg(long, requires = "endpoint")]
    pub token: Option<String>,
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

    /// Search indexed documents by keywords
    #[command(after_help = "\
Examples:
  linkly search \"attention\" --library my-research
  linkly search \"todo\" --scope notes --tags project
  linkly search \"source unclear\" --scope annotations     # only your clip annotations

Your own clip annotations (highlights, notes, comments from the clip reader)
are searched by default; each result says in `matched_in` whether it hit the
content, the annotations or both, and Markdown marks such snippets with
\"(in annotations)\".")]
    Search {
        /// Search query
        query: String,

        /// Maximum number of results (default: 20, max: 50)
        #[arg(long)]
        limit: Option<usize>,

        /// Filter by document types (comma-separated, e.g. pdf,md,xlsx,csv,docx,doc,pptx,epub,rtf)
        #[arg(long, value_delimiter = ',')]
        r#type: Option<Vec<String>>,

        /// Restrict search to a specific library. Local: a plain name or local://<id>. Cloud (--remote only): cloud://<owner>/<slug>
        #[arg(long, value_hint = clap::ValueHint::Other)]
        library: Option<String>,

        /// Glob substring-matched against the file path, case-sensitive (no leading/trailing * needed). * = any chars incl. /, ? = one char. Examples: '*.pdf', 'papers', '/abs/dir/'
        #[arg(long)]
        path_glob: Option<String>,

        /// Inclusive lower bound on file modification time (ISO 8601 UTC). Bare date or RFC 3339, e.g. '2024-01-01' or '2024-01-01T00:00:00Z'.
        #[arg(long)]
        modified_after: Option<String>,

        /// Inclusive upper bound on file modification time (ISO 8601 UTC). Same format as --modified-after.
        #[arg(long)]
        modified_before: Option<String>,

        /// Reorder by modification time: 'newest' (most recent first), 'oldest' (earliest first), or 'default' (relevance order, same as omitting the flag)
        #[arg(long, value_parser = ["default", "newest", "oldest"])]
        time_sort: Option<String>,

        /// Search scope: 'folder' (default) searches all indexed content, your clip annotations included; 'notes' restricts results to your markdown notes and IGNORES --library and --path-glob; 'annotations' matches only the text of your own clip annotations (keyword-only, no field:value clauses), so only documents carrying annotations are returned — the other filters still apply
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

        /// 'content' (default: matching lines with context) or 'count' (match totals only)
        #[arg(long)]
        mode: Option<String>,

        /// Maximum number of matches (default: 20, max: 100)
        #[arg(long)]
        limit: Option<usize>,

        /// Number of matches to skip for pagination
        #[arg(long)]
        offset: Option<usize>,

        /// Treat runs of whitespace as one. Omit for auto (on for PDF, off otherwise); true/false forces it
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
    #[command(after_help = "\
Examples:
  linkly read local://42
  linkly read local://42 --offset 201 --limit 200
  linkly read local://42 --annotations-offset 20 --limit 1   # next page of annotations only

For a Markdown clip that carries your annotations (highlights, notes and
comments from the clip reader), a \"User annotations\" block follows the
content on the first page (--offset 1) or whenever --annotations-offset is
given. Annotations page independently of the lines: 20 per call, and the
block reports total, has_more and next_annotations_offset.")]
    Read {
        /// Document IDs from search. Pass '-' to read the IDs from stdin, one per line
        #[arg(required = true)]
        ids: Vec<String>,

        /// Starting line number (1-based)
        #[arg(long)]
        offset: Option<usize>,

        /// Number of lines to read (default: 200, max: 500)
        #[arg(long)]
        limit: Option<usize>,

        /// Detail for the referenced-images block: 'none' (mapping only), 'abstract' (default: plus excerpt and word count), 'full' (plus inline OCR text)
        #[arg(long, value_parser = ["none", "abstract", "full"])]
        image_text: Option<String>,

        /// Offset into the document's user annotations (0-based, default 0). The "User annotations" block is appended on the first content page (--offset 1) or whenever this flag is given, 20 annotations per call; pass --limit 1 to page annotations only
        #[arg(long, value_name = "N")]
        annotations_offset: Option<usize>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// List files in a folder or library, or your notes (no full-text matching)
    ///
    /// Enumerates indexed files under a directory (--scope folder), one
    /// library's files (--scope library), or your notes (--scope notes). Notes
    /// live on the Desktop machine — with --remote this reads them through the
    /// tunnel, and there is no cloud notes store to fall back on when Desktop
    /// is offline
    List {
        /// Container scope to list. Required. 'folder' lists indexed files under a directory (omit --path to sweep all watched roots), 'library' lists one library's files (requires --library), 'notes' lists your markdown notes
        #[arg(long, value_hint = clap::ValueHint::Other)]
        scope: String,

        /// Which library to list (--scope library only; required there). Local: a plain name or local://<id>. Cloud (--remote only): cloud://<owner>/<slug>. See `linkly list-libraries`
        #[arg(long, value_hint = clap::ValueHint::Other)]
        library: Option<String>,

        /// Absolute directory path to list (--scope folder, or inside a local library). An address, not a glob — if you only know a fuzzy name, run `linkly find-paths` first
        #[arg(long, value_hint = clap::ValueHint::DirPath)]
        path: Option<String>,

        /// Filter by document types (comma-separated, e.g. pdf,md,xlsx,csv,docx,doc,pptx,epub,rtf) — --scope folder/library only
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

    /// Create or rewrite a markdown note on your Desktop
    ///
    /// Writes to the Notes folder of your Desktop. With --remote the write still
    /// lands on that machine, forwarded through the tunnel — notes are never
    /// stored in the cloud, so there is no cloud library to target and none to
    /// fall back on when Desktop is offline
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

        /// Display name shown as the note's source badge (e.g. 'Claude Code'); not the model name, omit if unsure, max 64 chars. Separate from --client (the access-log label): pass both
        #[arg(long, value_hint = clap::ValueHint::Other)]
        app_name: Option<String>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Show Desktop status: version, indexed documents, index state (gateway and tunnel with --remote)
    Status {
        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Diagnose connections: this machine's Desktop, plus remote and LAN when configured
    #[command(long_about = "\
Diagnose connections, one group per connection with advice for each failure.

Without a flag: checks the Desktop on this machine, plus the remote gateway and
the saved LAN Desktop when credentials for them exist, concurrently. Exits 0 when
at least one connection works — read each group's status for the one you need.
With --remote, --lan or --endpoint/--token: checks only that connection, and the
exit code reflects only it.")]
    Doctor {
        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Run as an MCP stdio bridge for MCP clients that only speak stdio
    ///
    /// For clients such as Claude Desktop. Default: the Linkly AI Desktop on
    /// this machine; --remote: the remote gateway (adds cloud:// libraries).
    /// There is no LAN mode: LAN clients connect to Desktop over HTTP. Calls
    /// carry the connecting application's own name (with --remote, to the
    /// gateway too); --client <app-name> overrides it.
    Mcp {
        /// Not supported: kept (hidden) only so that passing it fails with
        /// guidance instead of a bare "unexpected argument". LAN clients
        /// should connect to Desktop over HTTP directly.
        #[arg(long, hide = true, conflicts_with = "remote")]
        endpoint: Option<String>,

        /// Bridge through the remote gateway (https://mcp.linkly.ai): your Desktop plus linked cloud libraries. Needs a saved key (`linkly auth login` or `set-key`)
        #[arg(long, conflicts_with = "endpoint")]
        remote: bool,
    },

    /// Print a shell completion script
    #[command(long_about = "\
Print a shell completion script to stdout

The script is static: it completes subcommands, flags and fixed value sets. It
does not query Desktop, so it never blocks your prompt and works with
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

    /// Install, update, or inspect the linkly-ai agent skill
    #[command(long_about = "\
Install, update, or inspect the linkly-ai agent skill

When an update is available, command output carries a one-line notice, at most
once every four hours. Set LINKLY_NO_SKILLS_HINT=1 to turn it off. `linkly mcp` inherits
the environment, so setting it in an MCP client's config silences the bridge
too.")]
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
    },
}

#[derive(Subcommand)]
pub enum SkillsAction {
    /// Show the installed version, the latest published one, and where it lives
    Status,

    /// Download and install the skill
    Install,

    /// Update an installed skill, or install it when it is missing
    Update,
}

#[derive(Subcommand)]
pub enum AuthAction {
    /// Save an API key for remote access, or a LAN token with --lan
    #[command(after_help = "\
Examples:
  linkly auth set-key lkai_xxx                                          # remote (default)
  linkly auth set-key <token> --lan --endpoint http://192.168.1.100:60606  # LAN (/mcp optional)

Find the LAN address and token in Linkly AI Desktop > Agent usage > View MCP connection settings.")]
    SetKey {
        /// API key (format: lkai_...), or with --lan the Desktop's LAN token
        key: String,

        /// Save the key for remote access via https://mcp.linkly.ai (the default)
        #[arg(long, conflicts_with = "lan")]
        remote: bool,

        /// Save a LAN token instead, used by `--lan` on other commands
        #[arg(long, requires = "endpoint")]
        lan: bool,

        /// LAN Desktop address (e.g. http://192.168.1.100:60606), required with --lan
        #[arg(long, requires = "lan")]
        endpoint: Option<String>,
    },

    /// Sign in through the browser for --remote. Agents: --no-wait, then --wait (see --help)
    #[command(long_about = "\
Sign in through the browser and save a new API key for remote access

AGENTS: don't run plain `linkly auth login` — it waits in the foreground until
the link is approved, so the user never sees the link. Use two steps, in the
same environment and config directory:

  linkly auth login --no-wait --json   # returns url + user_code; send both to the user
  linkly auth login --wait --json      # after they approve; repeat on `timeout`

--wait gives up after 300 s; if your shell tool kills commands sooner, pass a
shorter --timeout. A killed or timed-out --wait doesn't cancel the login.

People at a terminal just run `linkly auth login`: it prints a one-time link
and a confirmation code, opens the browser, and stores a freshly issued key
once the link is approved (as `set-key` would). The link works on any device
and is valid for 10 minutes.

One config directory holds one account (see LINKLY_CONFIG_DIR). Failures carry
a code in --json mode: timeout, expired, denied, no_pending_login,
login_in_progress, already_logged_in, network.")]
    Login {
        /// Start the login and print the link immediately (agents: use this, send url + code to the user, then run --wait)
        #[arg(long, conflicts_with = "wait")]
        no_wait: bool,

        /// Finish a login started with --no-wait (can be run again after a timeout)
        #[arg(long, conflicts_with_all = ["restart", "no_browser"])]
        wait: bool,

        /// Stop this wait after SECONDS (default: until the link expires; 300 with --wait)
        #[arg(long, value_name = "SECONDS", conflicts_with = "no_wait")]
        timeout: Option<u64>,

        /// Abandon a login that is still waiting and start a new one
        #[arg(long)]
        restart: bool,

        /// Only print the link, don't try to open a browser
        #[arg(long)]
        no_browser: bool,
    },

    /// Show the stored credentials (remote key and LAN connection)
    Status,

    /// Remove the stored remote key, or the LAN connection with --lan.
    /// A key from `auth login` is also revoked on the server.
    Logout {
        /// Remove the remote API key (the default)
        #[arg(long, conflicts_with = "lan")]
        remote: bool,

        /// Remove the saved LAN connection instead
        #[arg(long)]
        lan: bool,
    },
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
        // The annotations scope reaches the desktop verbatim.
        let cli = parse(&["search", "q", "--scope", "annotations"]).expect("parse");
        let Command::Search { scope, .. } = cli.command else {
            panic!("expected Search, got something else");
        };
        assert_eq!(scope.as_deref(), Some("annotations"));
    }

    #[test]
    fn read_annotations_offset_is_an_optional_integer() {
        // Absent means "desktop default" (0), so it must stay None
        // rather than being sent as 0 — see read.rs on how args are built.
        let cli = parse(&["read", "local://1"]).expect("parse");
        let Command::Read {
            annotations_offset, ..
        } = cli.command
        else {
            panic!("expected Read, got something else");
        };
        assert_eq!(annotations_offset, None);

        let cli = parse(&[
            "read",
            "local://1",
            "--annotations-offset",
            "20",
            "--limit",
            "1",
        ])
        .expect("parse");
        let Command::Read {
            annotations_offset,
            limit,
            ..
        } = cli.command
        else {
            panic!("expected Read, got something else");
        };
        assert_eq!(annotations_offset, Some(20));
        assert_eq!(limit, Some(1));

        // 0-based offset: 0 is a valid first page; negatives and words are not.
        assert!(parse(&["read", "local://1", "--annotations-offset", "0"]).is_ok());
        assert!(parse(&["read", "local://1", "--annotations-offset", "-1"]).is_err());
        assert!(parse(&["read", "local://1", "--annotations-offset", "twenty"]).is_err());
        assert!(parse(&["read", "local://1", "--annotations-offset"]).is_err());
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

    #[test]
    fn auth_targets_remote_by_default_and_lan_on_request() {
        assert!(parse(&["auth", "set-key", "lkai_x", "--remote"]).is_ok());
        assert!(parse(&["auth", "set-key", "tk", "--lan", "--endpoint", "http://x:1"]).is_ok());
        assert!(parse(&["auth", "logout", "--remote"]).is_ok());
        assert!(parse(&["auth", "logout", "--lan"]).is_ok());
    }

    #[test]
    fn login_flags_combine_only_in_meaningful_ways() {
        assert!(parse(&["auth", "login"]).is_ok());
        assert!(parse(&["auth", "login", "--no-wait", "--restart", "--no-browser"]).is_ok());
        assert!(parse(&["auth", "login", "--wait", "--timeout", "10"]).is_ok());
        assert!(parse(&["auth", "login", "--timeout", "60"]).is_ok());
        assert!(parse(&["auth", "login", "--wait", "--no-wait"]).is_err());
        assert!(parse(&["auth", "login", "--wait", "--restart"]).is_err());
        assert!(parse(&["auth", "login", "--no-wait", "--timeout", "10"]).is_err());
    }

    #[test]
    fn auth_rejects_inconsistent_targets() {
        // --lan needs an address, and an address only makes sense with --lan.
        assert!(parse(&["auth", "set-key", "tk", "--lan"]).is_err());
        assert!(parse(&["auth", "set-key", "tk", "--endpoint", "http://x:1"]).is_err());
        assert!(parse(&[
            "auth",
            "set-key",
            "tk",
            "--remote",
            "--lan",
            "--endpoint",
            "http://x:1"
        ])
        .is_err());
        assert!(parse(&["auth", "logout", "--remote", "--lan"]).is_err());
    }

    #[test]
    fn mcp_endpoint_still_parses_so_it_can_fail_with_guidance() {
        // Hidden but accepted: the bridge rejects it at run time with an
        // explanation, which beats clap's "unexpected argument".
        assert!(parse(&["mcp", "--endpoint", "http://x:1/mcp"]).is_ok());
        assert!(parse(&["mcp", "--lan"]).is_err());
    }
}
