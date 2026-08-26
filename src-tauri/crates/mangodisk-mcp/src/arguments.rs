use clap::Parser;

/// MCP transport and safety options for the MangoDisk server adapter.
#[derive(Debug, Parser)]
#[command(
    name = "mangodisk-mcp",
    about = "MangoDisk Model Context Protocol server"
)]
pub struct Cli {
    /// Serve MCP over streamable HTTP on 127.0.0.1 instead of stdio.
    #[arg(long)]
    pub http: bool,

    /// Port for --http mode. 0 picks a random free port.
    #[arg(long, default_value_t = 0, requires = "http")]
    pub port: u16,

    /// Return full filesystem paths in tool responses. By default paths are
    /// redacted to a leaf name plus a short digest.
    #[arg(long)]
    pub include_full_paths: bool,

    /// Enable mutation tools. Same effect as MANGODISK_MCP_ENABLE_MUTATIONS=1.
    #[arg(long)]
    pub enable_mutations: bool,
}

pub const ENABLE_MUTATIONS_ENV: &str = "MANGODISK_MCP_ENABLE_MUTATIONS";

impl Cli {
    /// Mutations stay disabled unless the operator opts in through the flag or
    /// the environment. Both paths are checked so process supervisors that can
    /// only inject environment variables keep the same explicit gate.
    pub fn mutations_enabled(&self) -> bool {
        self.enable_mutations || std::env::var_os(ENABLE_MUTATIONS_ENV).is_some_and(|v| v == "1")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutations_require_an_explicit_opt_in() {
        let cli = Cli::try_parse_from(["mangodisk-mcp"]).expect("default arguments must parse");
        // The environment variable is process-global, so this test only
        // exercises the flag default.
        assert!(!cli.enable_mutations);
        assert!(!cli.http);
        assert_eq!(cli.port, 0);
        assert!(!cli.include_full_paths);
    }

    #[test]
    fn port_requires_http_mode() {
        assert!(Cli::try_parse_from(["mangodisk-mcp", "--port", "8080"]).is_err());
        assert!(Cli::try_parse_from(["mangodisk-mcp", "--http", "--port", "8080"]).is_ok());
    }
}
