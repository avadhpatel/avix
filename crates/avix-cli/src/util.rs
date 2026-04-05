use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;

use avix_client_core::atp::{AtpClient, Dispatcher};
use avix_client_core::config::ClientConfig;
use avix_core::config::{LlmConfig, ProviderAuth, ProviderConfig};
use avix_core::llm_client::LlmClient;
use avix_core::llm_svc::adapter::xai::XaiAdapter;
use avix_core::llm_svc::autoagents_client::AutoAgentsChatClient;
use avix_core::llm_svc::DirectHttpLlmClient;
use avix_core::types::Modality;

/// Emit output in JSON or human-readable format.
pub fn emit<T: serde::Serialize>(json_mode: bool, human_fn: impl FnOnce(&T) -> String, value: T) {
    if json_mode {
        println!("{}", serde_json::to_string(&value).unwrap());
    } else {
        println!("{}", human_fn(&value));
    }
}

pub fn format_catalog(agents: &Vec<serde_json::Value>) -> String {
    if agents.is_empty() {
        return "No agents installed.".to_string();
    }
    let mut out = format!(
        "{:<24} {:<10} {:<8} {}\n",
        "NAME", "VERSION", "SCOPE", "DESCRIPTION"
    );
    out.push_str(&"-".repeat(72));
    out.push('\n');
    for a in agents {
        let name = a["name"].as_str().unwrap_or("?");
        let version = a["version"].as_str().unwrap_or("?");
        let scope = a["scope"].as_str().unwrap_or("?");
        let desc = a["description"].as_str().unwrap_or("");
        out.push_str(&format!(
            "{:<24} {:<10} {:<8} {}\n",
            name, version, scope, desc
        ));
    }
    out
}

pub fn format_history(records: &Vec<serde_json::Value>) -> String {
    if records.is_empty() {
        return "No invocation history.".to_string();
    }
    let mut out = format!(
        "{:<12} {:<20} {:<12} {:<26} {}\n",
        "ID", "AGENT", "STATUS", "SPAWNED", "TOKENS"
    );
    out.push_str(&"-".repeat(80));
    out.push('\n');
    for r in records {
        let id = r["id"].as_str().unwrap_or("?");
        let short_id = if id.len() > 8 { &id[..8] } else { id };
        let agent = r["agentName"].as_str().unwrap_or("?");
        let status = r["status"].as_str().unwrap_or("?");
        let spawned = r["spawnedAt"].as_str().unwrap_or("?");
        let tokens = r["tokensConsumed"].as_u64().unwrap_or(0);
        out.push_str(&format!(
            "{:<12} {:<20} {:<12} {:<26} {}\n",
            short_id, agent, status, spawned, tokens
        ));
    }
    out
}

pub fn format_invocation(inv: &serde_json::Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("ID:      {}\n", inv["id"].as_str().unwrap_or("?")));
    out.push_str(&format!(
        "Agent:   {}\n",
        inv["agentName"].as_str().unwrap_or("?")
    ));
    out.push_str(&format!(
        "Status:  {}\n",
        inv["status"].as_str().unwrap_or("?")
    ));
    out.push_str(&format!(
        "Goal:    {}\n",
        inv["goal"].as_str().unwrap_or("")
    ));
    out.push_str(&format!(
        "Spawned: {}\n",
        inv["spawnedAt"].as_str().unwrap_or("?")
    ));
    if let Some(ended) = inv["endedAt"].as_str() {
        out.push_str(&format!("Ended:   {}\n", ended));
    }
    out.push_str(&format!(
        "Tokens:  {}\n",
        inv["tokensConsumed"].as_u64().unwrap_or(0)
    ));
    out.push('\n');
    if let Some(messages) = inv["conversation"].as_array() {
        out.push_str("--- Conversation ---\n");
        for msg in messages {
            let role = msg["role"].as_str().unwrap_or("?");
            let content = msg["content"].as_str().unwrap_or("");
            out.push_str(&format!("[{}] {}\n", role, content));
        }
    }
    out
}

pub fn expand_home(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path
}

pub async fn connect_config(
    config: Option<PathBuf>,
    server_url: Option<String>,
) -> Result<Dispatcher> {
    let mut cfg = ClientConfig::load_from(config).unwrap_or_else(|_| ClientConfig::default());
    if let Some(url) = server_url {
        cfg.server_url = url;
    }
    let client = AtpClient::connect(cfg).await?;
    Ok(Dispatcher::new(client))
}

pub fn owner_from(for_service: Option<String>, for_user: Option<String>) -> Result<String> {
    match (for_service, for_user) {
        (Some(svc), _) => Ok(format!("service:{svc}")),
        (_, Some(user)) => Ok(format!("user:{user}")),
        _ => anyhow::bail!("specify --for-service <name> or --for-user <name>"),
    }
}

pub fn load_llm_config(root: &std::path::Path) -> Result<LlmConfig> {
    let path = root.join("etc/llm.yaml");
    let src = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    LlmConfig::from_str(&src).map_err(|e| anyhow::anyhow!("{e}"))
}

pub fn default_text_model(provider: &ProviderConfig) -> Option<String> {
    let text_models: Vec<_> = provider
        .models
        .iter()
        .filter(|m| m.modality == Modality::Text)
        .collect();
    text_models
        .iter()
        .find(|m| m.tier == "standard")
        .or_else(|| text_models.first())
        .map(|m| m.id.clone())
}

pub fn build_llm_client(provider: &ProviderConfig, model: &str) -> Result<Box<dyn LlmClient>> {
    let api_key = match &provider.auth {
        ProviderAuth::ApiKey { secret_name, .. } => {
            Some(std::env::var(secret_name).with_context(|| {
                format!(
                    "{secret_name} not set — set this env var with your {} API key",
                    provider.name
                )
            })?)
        }
        ProviderAuth::None => None,
        ProviderAuth::Oauth2 { .. } => {
            return Err(anyhow::anyhow!(
                "OAuth2 providers are not yet supported in CLI mode"
            ))
        }
    };

    match provider.name.as_str() {
        "anthropic" => {
            use autoagents::llm::backends::anthropic::Anthropic;
            use autoagents::llm::builder::LLMBuilder;
            let p = LLMBuilder::<Anthropic>::new()
                .api_key(api_key.unwrap_or_default())
                .model(model)
                .max_tokens(4096)
                .build()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Box::new(AutoAgentsChatClient::new(p)))
        }
        "openai" => {
            use autoagents::llm::backends::openai::OpenAI;
            use autoagents::llm::builder::LLMBuilder;
            let p = LLMBuilder::<OpenAI>::new()
                .api_key(api_key.unwrap_or_default())
                .model(model)
                .max_tokens(4096)
                .build()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Box::new(AutoAgentsChatClient::new(p)))
        }
        "xai" => {
            let auth = api_key.map(|k| ("Authorization".to_string(), format!("Bearer {k}")));
            Ok(Box::new(DirectHttpLlmClient::new(
                "https://api.x.ai",
                model,
                auth,
                Arc::new(XaiAdapter::new()),
            )))
        }
        "ollama" => {
            use autoagents::llm::backends::ollama::Ollama;
            use autoagents::llm::builder::LLMBuilder;
            let p = LLMBuilder::<Ollama>::new()
                .model(model)
                .max_tokens(4096)
                .build()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Box::new(AutoAgentsChatClient::new(p)))
        }
        other => Err(anyhow::anyhow!(
            "unsupported provider '{}' — supported: anthropic, openai, xai, ollama",
            other
        )),
    }
}

pub async fn run_atp_shell(server_url: String, token: Option<String>) -> Result<()> {
    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use std::io::{self, Write};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{client::IntoClientRequest, Message},
    };

    println!("ATP Shell — connecting to {}", server_url);

    let credential = if let Some(t) = token {
        t
    } else {
        print!("Credential: ");
        io::stdout().flush()?;
        let mut credential = String::new();
        io::stdin().read_line(&mut credential)?;
        credential.trim().to_string()
    };

    let client = reqwest::Client::new();
    let login_url = server_url
        .replace("ws://", "http://")
        .replace("wss://", "https://")
        .replace("/atp", "/atp/auth/login");
    let resp = client
        .post(&login_url)
        .json(&serde_json::json!({"identity": "test", "credential": credential}))
        .send()
        .await?;
    let body: Value = resp.json().await?;
    let token = body["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Login failed: {:?}", body))?
        .to_string();

    println!("Logged in, connecting WS...");

    let mut request = server_url.into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {}", token).parse()?);
    let (ws_stream, _) = connect_async(request).await?;
    let (mut write, mut read) = ws_stream.split();

    let sub_msg = serde_json::json!({"type": "subscribe", "events": ["*"]});
    write.send(Message::Text(sub_msg.to_string())).await?;

    println!("Connected. Type JSON-RPC commands, or 'help', 'quit'.");
    println!("Events will be printed as received.");

    let event_handle = tokio::spawn(async move {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(event) = serde_json::from_str::<Value>(&text) {
                        if event.get("type").is_some() && event["type"] != "reply" {
                            println!("EVENT: {}", serde_json::to_string_pretty(&event).unwrap());
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(e) => eprintln!("WS error: {}", e),
                _ => {}
            }
        }
    });

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    loop {
        print!("atp> ");
        stdout.flush()?;
        let mut line = String::new();
        stdin.read_line(&mut line)?;
        let line = line.trim();

        match line {
            "" => continue,
            "quit" | "exit" => break,
            "help" => {
                println!("Commands:");
                println!("  <json>  - Send JSON-RPC request");
                println!("  help    - This help");
                println!("  quit    - Exit");
                continue;
            }
            _ => match serde_json::from_str::<Value>(line) {
                Ok(mut req) => {
                    static mut ID: u64 = 0;
                    unsafe { ID += 1 };
                    req["jsonrpc"] = "2.0".into();
                    req["id"] = unsafe { ID }.into();
                    write.send(Message::Text(req.to_string())).await?;
                    println!("Sent: {}", req);
                }
                Err(_) => {
                    eprintln!("Invalid JSON. Try: {{\"method\": \"proc.list\", \"params\": {{}}}}");
                }
            },
        }
    }

    write.send(Message::Close(None)).await?;
    event_handle.abort();
    Ok(())
}
