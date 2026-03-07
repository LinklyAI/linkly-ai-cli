use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "linkly",
    version,
    about = "Linkly AI — search your local documents from the terminal"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// MCP endpoint URL (e.g. http://127.0.0.1:60606/mcp)
    #[arg(long, global = true)]
    pub endpoint: Option<String>,

    /// Output in JSON format
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum Command {
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
    },

    /// Get document outlines by IDs
    Outline {
        /// Document IDs (from search results)
        #[arg(required = true)]
        ids: Vec<String>,
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
    },

    /// Show Linkly AI app status
    Status,

    /// Run as MCP stdio bridge (for Claude Desktop, etc.)
    Mcp,

    /// Update to the latest version
    SelfUpdate,
}
