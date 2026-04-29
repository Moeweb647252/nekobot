#[tokio::main]
async fn main() {
    let config_content = tokio::fs::read_to_string("config.yaml")
        .await
        .expect("Failed to read config.yaml");
    let config = serde_json::from_str(&config_content).expect("Failed to parse config.yaml");
    let mut bot = nekobot_core::NekoBot::new(config);
    nekobot_provider::register_providers(bot.provider_registry_mut())
        .expect("Failed to register providers");
    if let Err(e) = bot.run().await {
        eprintln!("Error running NekoBot: {e}");
    }
}
