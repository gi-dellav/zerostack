#[cfg(test)]
mod tests {
    use crate::cli::Cli;
    use crate::config::Config;
    use crate::extras::acp::config::AcpServerConfig;
    use crate::extras::acp::resolve_acp_mode;
    use crate::permission::SecurityMode;

    #[test]
    fn test_acp_config_tcp_deserialization() {
        let json = r#"{"type":"tcp","host":"0.0.0.0","port":7243}"#;
        let cfg: AcpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.transport_type(), "tcp");
    }

    #[test]
    fn test_acp_config_tcp_with_api_key() {
        let json = r#"{"type":"tcp","host":"127.0.0.1","port":9999,"api_key":"secret"}"#;
        let cfg: AcpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.transport_type(), "tcp");
    }

    #[test]
    fn test_acp_config_stdio() {
        let json = r#"{"type":"stdio"}"#;
        let cfg: AcpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.transport_type(), "stdio");
    }

    #[test]
    fn test_acp_config_unknown_type_errors() {
        let json = r#"{"type":"http","url":"https://example.com"}"#;
        let result: Result<AcpServerConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_acp_cli_defaults() {
        let cli = Cli::default();
        assert!(!cli.acp_enabled);
        assert!(cli.acp_host.is_none());
        assert!(cli.acp_port.is_none());
    }

    #[test]
    fn test_acp_cli_tcp_config() {
        let cli = Cli {
            acp_enabled: true,
            acp_host: Some("0.0.0.0".into()),
            acp_port: Some(7243),
            ..Default::default()
        };
        assert!(cli.acp_enabled);
        assert_eq!(cli.acp_host.as_deref(), Some("0.0.0.0"));
        assert_eq!(cli.acp_port, Some(7243));
    }

    #[test]
    fn test_acp_config_default_fields() {
        let cfg = Config::default();
        assert!(cfg.acp_servers.is_none());
        assert!(cfg.acp_host.is_none());
        assert!(cfg.acp_port.is_none());
    }

    #[test]
    fn test_security_mode_discriminants() {
        use SecurityMode::*;
        let modes = [Yolo, Standard, Guarded, ReadOnly, Restrictive];
        assert_eq!(modes.len(), 5);
        assert!(matches!(SecurityMode::Yolo, SecurityMode::Yolo));
    }

    // ── resolve_acp_mode tests ──────────────────────────────────────

    #[test]
    fn test_resolve_acp_mode_yolo_cli() {
        let cli = Cli {
            yolo: true,
            ..Default::default()
        };
        let cfg = Config::default();
        assert_eq!(resolve_acp_mode(&cli, &cfg), SecurityMode::Yolo);
    }

    #[test]
    fn test_resolve_acp_mode_accept_all() {
        let cli = Cli {
            accept_all: true,
            ..Default::default()
        };
        let cfg = Config::default();
        assert_eq!(resolve_acp_mode(&cli, &cfg), SecurityMode::Standard);
    }

    #[test]
    fn test_resolve_acp_mode_restrictive() {
        let cli = Cli {
            restrictive: true,
            ..Default::default()
        };
        let cfg = Config::default();
        assert_eq!(resolve_acp_mode(&cli, &cfg), SecurityMode::Restrictive);
    }

    #[test]
    fn test_resolve_acp_mode_default_standard() {
        let cli = Cli::default();
        let cfg = Config::default();
        assert_eq!(resolve_acp_mode(&cli, &cfg), SecurityMode::Standard);
    }

    #[test]
    fn test_resolve_acp_mode_yolo_config() {
        let cli = Cli::default();
        let cfg = Config {
            yolo: Some(true),
            ..Default::default()
        };
        assert_eq!(resolve_acp_mode(&cli, &cfg), SecurityMode::Yolo);
    }

    #[test]
    fn test_resolve_acp_mode_config_default_mode() {
        let cli = Cli::default();
        let cfg = Config {
            default_permission_mode: Some("guarded".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_acp_mode(&cli, &cfg), SecurityMode::Guarded);
    }

    #[test]
    fn test_resolve_acp_mode_skip_permissions() {
        let cli = Cli {
            dangerously_skip_permissions: true,
            ..Default::default()
        };
        let cfg = Config::default();
        assert_eq!(resolve_acp_mode(&cli, &cfg), SecurityMode::Standard);
    }

    #[test]
    fn test_resolve_acp_mode_cli_overrides_config() {
        let cli = Cli {
            yolo: true,
            ..Default::default()
        };
        let cfg = Config {
            default_permission_mode: Some("restrictive".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_acp_mode(&cli, &cfg), SecurityMode::Yolo);
    }

    #[test]
    fn test_format_mcp_tool_title() {
        use crate::extras::acp::format_mcp_tool_title;
        assert_eq!(
            format_mcp_tool_title("chat", "send_message"),
            "mcp__chat__send_message"
        );
        assert_eq!(
            format_mcp_tool_title("codync-memory", "remember"),
            "mcp__codync_memory__remember"
        );
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn test_acp_server_to_mcp_config_stdio_and_http() {
        use std::path::PathBuf;

        use agent_client_protocol::v1::schema::{
            EnvVariable, HttpHeader, McpServer, McpServerHttp, McpServerStdio,
        };

        use crate::extras::acp::{acp_server_to_mcp_config, build_acp_mcp_configs};
        use crate::extras::mcp::config::McpServerConfig;

        let stdio = McpServer::Stdio(
            McpServerStdio::new("chat", PathBuf::from("/usr/bin/codync-host"))
                .args(vec!["mcp".to_string(), "chat".to_string()])
                .env(vec![EnvVariable::new("CODYNC_HOME", "/tmp/codync")]),
        );
        let (name, cfg) = acp_server_to_mcp_config(&stdio).expect("stdio config");
        assert_eq!(name, "chat");
        match cfg {
            McpServerConfig::Command {
                command, args, env, ..
            } => {
                assert_eq!(command, "/usr/bin/codync-host");
                assert_eq!(args, vec!["mcp", "chat"]);
                assert_eq!(
                    env.get("CODYNC_HOME").map(String::as_str),
                    Some("/tmp/codync")
                );
            }
            _ => panic!("expected Command config"),
        }

        let http = McpServer::Http(
            McpServerHttp::new("remote", "https://example.com/mcp")
                .headers(vec![HttpHeader::new("Authorization", "Bearer token")]),
        );
        let merged = build_acp_mcp_configs(&Config::default(), &[stdio, http]);
        assert!(merged.contains_key("chat"));
        assert!(merged.contains_key("remote"));
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn test_effective_acp_cli_preserves_mcp_tools() {
        use std::collections::HashMap;

        use crate::extras::acp::effective_acp_cli;

        let mut titles = HashMap::new();
        titles.insert(
            "send_message".to_string(),
            "mcp__chat__send_message".to_string(),
        );
        let cli = Cli {
            tools: vec!["read".to_string()],
            ..Default::default()
        };
        let eff = effective_acp_cli(&cli, &Config::default(), &titles);
        assert!(eff.tools.contains(&"read".to_string()));
        assert!(eff.tools.contains(&"send_message".to_string()));
    }
}
