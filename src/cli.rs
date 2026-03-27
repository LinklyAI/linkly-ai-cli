use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "linkly",
    version,
    about = "Linkly AI — search your local documents from the terminal"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Output in JSON format
    #[arg(long, global = true)]
    pub json: bool,
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

    /// Connect via remote tunnel (https://mcp.linkly.ai)
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

    /// Search indexed local documents by keywords
    Search {
        /// Search query
        query: String,

        /// Maximum number of results (default: 20, max: 50)
        #[arg(long)]
        limit: Option<usize>,

        /// Filter by document types (comma-separated, e.g. pdf,md,docx)
        #[arg(long, value_delimiter = ',')]
        r#type: Option<Vec<String>>,

        /// Restrict search to a specific library by name
        #[arg(long)]
        library: Option<String>,

        /// Glob pattern to filter by file path (e.g. '*.pdf', '*papers*')
        #[arg(long)]
        path_glob: Option<String>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Locate specific lines within a document by regex pattern
    Grep {
        /// Regular expression pattern
        pattern: String,

        /// Document ID to search within (from search results)
        doc_id: String,

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
        /// Document IDs (from search results)
        #[arg(required = true)]
        ids: Vec<String>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Read document content by ID
    Read {
        /// Document ID (from search results)
        id: String,

        /// Starting line number (1-based)
        #[arg(long)]
        offset: Option<usize>,

        /// Number of lines to read (max: 500)
        #[arg(long)]
        limit: Option<usize>,

        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Show Linkly AI app status
    Status {
        #[command(flatten)]
        conn: ConnectionArgs,
    },

    /// Run as MCP stdio bridge (for Claude Desktop, etc.). Only supports local and LAN modes; use --endpoint for LAN.
    Mcp {
        /// MCP endpoint URL (e.g. http://192.168.1.100:60606/mcp)
        #[arg(long)]
        endpoint: Option<String>,
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
}
