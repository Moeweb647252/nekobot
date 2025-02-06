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
redis_url = ""
password = ""
context_length = 0
log_level = ""
system_prompt = """"""
enable_msg = ""

[bot]
token = ""
proxy = "" (optional)

[text]
api_key = ""
api_base = ""
model = ""
provider = "" 
proxy = "" (optional)
temperature = 0.0
max_tokens = 0
top_p = 0.0

```

Provider: openai for openai-compatible apis

## Building

```bash
cargo build --release
```

## Command

/retry : retry to generate completion
/regenerate : regenerate 

## License

MIT