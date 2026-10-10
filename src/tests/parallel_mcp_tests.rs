//! Exercise the recommended preset through the maintained HTTP tool path.
use std::sync::Arc;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::config::{Config, inject_mcp_defaults};
use crate::extras::mcp::{McpClientManager, config::McpServerConfig};

fn parallel_only_config() -> Config {
    let mut cfg: Config =
        toml::from_str("enable-parallel-mcp = true\nenable-exa-mcp = false").unwrap();
    inject_mcp_defaults(&mut cfg);
    assert_eq!(cfg.mcp_servers.as_ref().unwrap().len(), 1);
    cfg
}

async fn execute_search_and_fetch(cfg: Config) {
    let manager = McpClientManager::connect_all(cfg.mcp_servers.as_ref().unwrap()).await;
    assert!(manager.notices.is_empty(), "{:?}", manager.notices);
    let tools = manager.collect_tools(None, None).await;
    // Use zerostack's headless agent loop and the same dynamic ToolServer
    // registration as the production builder. Only model inference is scripted.
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    use crate::tests::fake_model::{MockCompletionModel, MockStreamEvent};
    let model = MockCompletionModel::from_stream_turns(vec![
        vec![
            MockStreamEvent::tool_call(
                "search-1",
                "web_search",
                json!({
                    "objective": "Find the official Rust language website",
                    "search_queries": ["Rust programming language official website"]
                }),
            ),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::tool_call(
                "fetch-1",
                "web_fetch",
                json!({
                    "urls": ["https://www.rust-lang.org/"],
                    "objective": "What is the Rust programming language?"
                }),
            ),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("Rust is a programming language.".to_string()),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);
    let captured = model.clone();
    let mut server = rig::tool::server::ToolServer::new();
    for tool in tools {
        server = server.dynamic_tool(tool);
    }
    let agent = rig::agent::AgentBuilder::new(model)
        .tool_server_handle(server.run())
        .default_max_turns(3)
        .build();
    let outcome = crate::agent::runner::run_print(
        &agent,
        "Search for the official Rust website and fetch its description.",
        true,
        &crate::retry::RetryConfig::default(),
        Vec::new(),
        #[cfg(feature = "hooks")]
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome.tool_interactions.len(), 2);
    for interaction in &outcome.tool_interactions {
        assert!(interaction.output.to_lowercase().contains("rust"));
        println!("{}: {}", interaction.name, interaction.output);
    }
    assert_eq!(captured.requests().len(), 3);
    let final_request = format!("{:?}", captured.requests()[2].chat_history);
    assert!(final_request.contains("rust-lang.org"));
    drop(agent);
    manager.shutdown().await;
}

#[tokio::test]
async fn parallel_mcp_http_dispatch_sends_project_user_agent() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let requests = seen.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let requests = requests.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let split = loop {
                    let mut chunk = [0; 4096];
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(i) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let headers = String::from_utf8(bytes[..split].to_vec()).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                while bytes.len() < split + length {
                    let mut chunk = [0; 4096];
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                if !headers.starts_with("POST /mcp ") {
                    socket.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                    return;
                }
                assert!(
                    headers
                        .lines()
                        .any(|line| line.eq_ignore_ascii_case(&format!(
                            "user-agent: zerostack/{}",
                            env!("CARGO_PKG_VERSION")
                        )))
                );
                assert!(!headers.to_lowercase().contains("authorization:"));
                let request: serde_json::Value =
                    serde_json::from_slice(&bytes[split..split + length]).unwrap();
                requests.lock().await.push(request.clone());
                let Some(id) = request.get("id") else {
                    socket.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                    return;
                };
                let result = match request["method"].as_str().unwrap() {
                    "initialize" => {
                        json!({"protocolVersion": request["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "fixture", "version": "1"}})
                    }
                    "tools/list" => json!({"tools": [
                        {"name": "web_search", "description": "search", "inputSchema": {"type": "object", "properties": {"objective": {"type": "string"}, "search_queries": {"type": "array", "items": {"type": "string"}}}, "required": ["objective", "search_queries"]}},
                        {"name": "web_fetch", "description": "fetch", "inputSchema": {"type": "object", "properties": {"urls": {"type": "array", "items": {"type": "string"}}}, "required": ["urls"]}}
                    ]}),
                    "tools/call" => {
                        json!({"content": [{"type": "text", "text": "Rust language: https://www.rust-lang.org/"}], "isError": false})
                    }
                    method => panic!("unexpected method: {method}"),
                };
                let body = json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
        }
    });
    let mut cfg = parallel_only_config();
    if let McpServerConfig::Url { url, .. } = cfg
        .mcp_servers
        .as_mut()
        .unwrap()
        .get_mut("Parallel")
        .unwrap()
    {
        assert_eq!(url, "https://search.parallel.ai/mcp");
        *url = format!("http://{address}/mcp");
    }
    execute_search_and_fetch(cfg).await;
    let requests = seen.lock().await;
    for method in ["initialize", "tools/list", "tools/call"] {
        assert!(requests.iter().any(|request| request["method"] == method));
    }
    let calls: Vec<_> = requests
        .iter()
        .filter(|request| request["method"] == "tools/call")
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["params"]["name"], "web_search");
    assert_eq!(
        calls[1]["params"]["arguments"]["urls"][0],
        "https://www.rust-lang.org/"
    );
    server.abort();
}

/// Optional real-server smoke test; never runs in normal CI or needs an API key.
#[tokio::test]
#[ignore = "requires internet access to the anonymous Parallel endpoint"]
async fn parallel_mcp_live_search_and_fetch() {
    execute_search_and_fetch(parallel_only_config()).await;
}
