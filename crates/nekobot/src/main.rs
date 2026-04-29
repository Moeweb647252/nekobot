//! NekoBot — modular multi-agent chatbot.
//!
//! Bootstrap flow:
//! 1. Load and parse `config.yaml`
//! 2. Create [`NekoBot`](nekobot_core::NekoBot) from the config
//! 3. Register provider implementations (DeepSeek, OpenAI Codex)
//! 4. Run the system

#[tokio::main]
async fn main() {
    // Read and parse config
    let config_content = tokio::fs::read_to_string("config.yaml")
        .await
        .expect("Failed to read config.yaml");
    let config = serde_json::from_str(&config_content).expect("Failed to parse config.yaml");

    // Build NekoBot with the config
    let mut bot = nekobot_core::NekoBot::new(config);

    // Register concrete provider implementations
    nekobot_provider::register_providers(bot.provider_registry_mut())
        .expect("Failed to register providers");

    // Start the bot (connects channels, runs agents)
    if let Err(e) = bot.run().await {
        eprintln!("Error running NekoBot: {e}");
    }
}
