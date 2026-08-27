use clap::Parser;

/// MCP transport and safety options for the MangoDisk server adapter.
#[derive(Debug, Parser)]
#[command(
    name = "mangodisk-mcp",
    about = "MangoDisk Model Context Protocol server"
)]
pub struct Cli {
    /// Serve MCP over streamable HTTP instead of stdio. Binds loopback unless
    /// --bind says otherwise.
    #[arg(long)]
    pub http: bool,

    /// Address for --http mode. Defaults to loopback; a non-loopback address
    /// exposes the server to the network. Bearer auth still applies, but the
    /// traffic is not encrypted, so only widen this on a trusted LAN or behind
    /// a tunnel/TLS terminator.
    #[arg(long, default_value = "127.0.0.1", requires = "http")]
    pub bind: std::net::IpAddr,

    /// Port for --http mode. 0 picks a random free port.
    #[arg(long, default_value_t = 0, requires = "http")]
    pub port: u16,

    /// Extra Host name the HTTP server accepts (repeatable), e.g. the LAN
    /// hostname clients dial. Loopback names and the bind address are always
    /// allowed; binding an unspecified address (0.0.0.0/::) disables this
    /// DNS-rebinding check because the dialed host cannot be enumerated.
    #[arg(long = "allowed-host", requires = "http")]
    pub allowed_hosts: Vec<String>,

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

    #[test]
    fn bind_defaults_to_loopback_and_requires_http_mode() {
        let cli = Cli::try_parse_from(["mangodisk-mcp", "--http"]).expect("http mode must parse");
        assert!(cli.bind.is_loopback());
        assert!(Cli::try_parse_from(["mangodisk-mcp", "--bind", "0.0.0.0"]).is_err());
        let lan = Cli::try_parse_from(["mangodisk-mcp", "--http", "--bind", "0.0.0.0"])
            .expect("http mode with an explicit bind must parse");
        assert!(lan.bind.is_unspecified());
    }

    #[test]
    fn allowed_hosts_repeat_and_require_http_mode() {
        assert!(Cli::try_parse_from(["mangodisk-mcp", "--allowed-host", "nas.local"]).is_err());
        let cli = Cli::try_parse_from([
            "mangodisk-mcp",
            "--http",
            "--allowed-host",
            "nas.local",
            "--allowed-host",
            "192.168.1.5:3939",
        ])
        .expect("repeatable allowed hosts must parse");
        assert_eq!(cli.allowed_hosts, ["nas.local", "192.168.1.5:3939"]);
    }
}
