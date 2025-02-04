# NekoBot

A Telegram chatbot.

## Features

- Chat with GPT models using Telegram interface
- Configurable via configuration file 
- Redis-backed storage for chat history

## Usage

```bash
nekobot -c config.toml
```

## Authentication

Directly send password to bot

### Configuration

Create a `config.toml` file with:

```toml
llm_api_key = ""
llm_api_base = ""
llm_model = ""
redis_url = ""
bot_token = ""
password = ""
context_length = 0
log_level = ""
system_prompt = """"""

```

## Building

```bash
cargo build --release
```

## Command

/retry : retry to generate completion
/regenerate : regenerate 

## License

MIT