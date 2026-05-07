//! NekoBot — modular multi-agent chatbot.
//!
//! Bootstrap flow:
//! 1. Load and parse `config.yaml`
//! 2. Create [`NekoBot`](nekobot_core::NekoBot) from the config
//! 3. Register channel implementations (QQ Bot, WeiXin)
//! 4. Register provider implementations (DeepSeek, OpenAI Codex)
//! 5. Register middleware factories (mcp, script, skills, tools, memory, persona)
//! 6. Run the system

macro_rules! register_middleware {
    ($bot:expr, $name:literal, $cfg_type:ty, $factory:expr) => {
        $bot.middleware_registry_mut()
            .register($name, |config| {
                let cfg: $cfg_type =
                    serde_json::from_value(serde_json::Value::Object(config.data.clone()))?;
                Ok(std::sync::Arc::new($factory(cfg))
                    as std::sync::Arc<
                        dyn nekobot_core::agent::middleware::Middleware,
                    >)
            })
            .expect(concat!("Failed to register ", $name, " middleware"));
    };
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
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
    bot.channel_registry_mut()
        .register("QQ", |cfg| match cfg {
            nekobot_core::config::ChannelConfig::QQ {
                name,
                app_id,
                client_secret,
                ..
            } => Ok(Box::new(nekobot_channel::channel::QQChannel::new(
                name.clone(),
                app_id.clone(),
                client_secret.clone(),
            )) as Box<dyn nekobot_channel::Channel>),
            nekobot_core::config::ChannelConfig::WeiXin { .. } => {
                anyhow::bail!("QQ factory received WeiXin config");
            }
        })
        .expect("Failed to register QQ channel");

    bot.channel_registry_mut()
        .register("WeiXin", |cfg| match cfg {
            nekobot_core::config::ChannelConfig::WeiXin { name, base_url, .. } => Ok(Box::new(
                nekobot_channel::channel::WeiXinChannel::new(name.clone(), base_url.clone()),
            )
                as Box<dyn nekobot_channel::Channel>),
            nekobot_core::config::ChannelConfig::QQ { .. } => {
                anyhow::bail!("WeiXin factory received QQ config");
            }
        })
        .expect("Failed to register WeiXin channel");

    // Register concrete provider implementations
    nekobot_provider::register_providers(bot.provider_registry_mut())
        .expect("Failed to register providers");

    // Register middleware factories
    register_middleware!(
        bot,
        "mcp",
        nekobot_mcp::McpConfig,
        nekobot_mcp::McpMiddleware::from_config
    );
    register_middleware!(
        bot,
        "script",
        nekobot_script::ScriptConfig,
        nekobot_script::ScriptMiddleware::from_config
    );
    register_middleware!(bot, "skills", nekobot_skills::SkillConfig, |c| {
        nekobot_skills::SkillMiddleware::from_config(c).unwrap()
    });
    register_middleware!(
        bot,
        "tools",
        nekobot_tools::ToolsConfig,
        nekobot_tools::ToolsMiddleware::from_config
    );
    register_middleware!(
        bot,
        "memory",
        nekobot_memory::MemoryConfig,
        nekobot_memory::MemoryMiddleware::from_config
    );

    // Persona has no config
    bot.middleware_registry_mut()
        .register("persona", |_config| {
            Ok(
                std::sync::Arc::new(nekobot_persona::PersonaMiddleware::new())
                    as std::sync::Arc<dyn nekobot_core::agent::middleware::Middleware>,
            )
        })
        .expect("Failed to register persona middleware");

    // Start the bot (connects channels, runs agents)
    if let Err(e) = bot.run().await {
        tracing::error!("NekoBot exited: {e}");
    }
}
