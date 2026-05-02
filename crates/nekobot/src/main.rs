//! NekoBot — modular multi-agent chatbot.
//!
//! Bootstrap flow:
//! 1. Load and parse `config.yaml`
//! 2. Create [`NekoBot`](nekobot_core::NekoBot) from the config
//! 3. Register channel implementations (QQ Bot)
//! 4. Register provider implementations (DeepSeek, OpenAI Codex)
//! 5. Run the system

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Read and parse config
    let config_content = tokio::fs::read_to_string("config.yaml")
        .await
        .expect("Failed to read config.yaml");
    let config: nekobot_core::config::Config =
        serde_yml::from_str(&config_content).expect("Failed to parse config.yaml");

    // Build NekoBot with the config
    let mut bot = nekobot_core::NekoBot::new(config);

    // Register concrete channel implementations
    bot.channel_registry_mut().register(
        "QQ",
        |cfg| match cfg {
            nekobot_core::config::ChannelConfig::QQ {
                app_id,
                client_secret,
                ..
            } => Ok(Box::new(nekobot_channel::channel::QQChannel::new(
                app_id.clone(),
                client_secret.clone(),
            )) as Box<dyn nekobot_channel::Channel>),
        },
    )
    .expect("Failed to register QQ channel");

    // Register concrete provider implementations
    nekobot_provider::register_providers(bot.provider_registry_mut())
        .expect("Failed to register providers");

    // Register MCP middleware factory
    bot.middleware_registry_mut()
        .register("mcp", |config| {
            tracing::info!(target: "mcp", "creating MCP middleware from data: {:?}", config.data);
            let mcp: nekobot_mcp::McpConfig = serde_json::from_value(
                serde_json::Value::Object(config.data.clone()),
            )
            .map_err(|e| {
                tracing::error!(target: "mcp", "failed to parse MCP config: {e}");
                anyhow::anyhow!("failed to parse MCP config: {e}")
            })?;
            Ok(std::sync::Arc::new(nekobot_mcp::McpMiddleware::from_config(mcp))
                as std::sync::Arc<dyn nekobot_core::agent::middleware::Middleware>)
        })
        .expect("Failed to register MCP middleware");

    // Register script middleware factory (eval_ts tool)
    bot.middleware_registry_mut()
        .register("script", |config| {
            let cfg: nekobot_script::ScriptConfig = serde_json::from_value(
                serde_json::Value::Object(config.data.clone()),
            )?;
            Ok(std::sync::Arc::new(
                nekobot_script::ScriptMiddleware::from_config(cfg),
            ) as std::sync::Arc<dyn nekobot_core::agent::middleware::Middleware>)
        })
        .expect("Failed to register script middleware");

    // Register skill middleware factory (Agent Skills support)
    bot.middleware_registry_mut()
        .register("skills", |config| {
            let cfg: nekobot_skills::SkillConfig = serde_json::from_value(
                serde_json::Value::Object(config.data.clone()),
            )?;
            Ok(std::sync::Arc::new(
                nekobot_skills::SkillMiddleware::from_config(cfg)?,
            ) as std::sync::Arc<dyn nekobot_core::agent::middleware::Middleware>)
        })
        .expect("Failed to register skill middleware");

    // Start the bot (connects channels, runs agents)
    if let Err(e) = bot.run().await {
        tracing::error!("NekoBot exited: {e}");
    }
}
