# Provider model architecture

Codex treats a provider and its model list as one configuration boundary.
Built-in OpenAI providers continue to use the existing Codex-managed model
catalog and are not mutated by provider-management UI. Custom
OpenAI-compatible providers own their model namespace: models fetched from a
custom provider are cached under that provider entry in `config.toml` and are
used as the selectable catalog whenever that provider is active.

## Config shape

A custom provider is stored under `model_providers.<provider_id>` with the
connection details and an optional `models` array:

```toml
model_provider = "deepseek"
model = "deepseek-chat"

[model_providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
env_key = "DEEPSEEK_API_KEY"
wire_api = "chat"

[[model_providers.deepseek.models]]
model_id = "deepseek-chat"
model_name = "DeepSeek Chat"
max_token_len = 64000
max_output_tokens = 8000
show_in_picker = true
```

`model_provider` and `model` remain the active-session selection. The provider
entry stores reusable connection metadata and the cached models for that one
provider. This keeps third-party models independent from OpenAI's built-in
catalog and prevents models from one custom provider appearing while another
provider is selected.

## TUI flow

The `/providers` manager separates providers into two groups:

1. **Built-in providers**: OpenAI providers managed by Codex. Their existing
   provider and model behavior is unchanged.
2. **Custom providers**: user-managed OpenAI-compatible providers. Their detail
   screen exposes **Fetch models**, which selects the provider, bypasses the
   cached model list, fetches the provider's current `/models` response, saves
   the resulting model metadata under that provider, and opens `/models` so the
   user can choose one of those provider-scoped models.

The `/models` picker only receives the model catalog for the currently selected
provider. If the user needs models from another provider, they should switch
providers first and fetch or select that provider's cached model list.

## Runtime loading

When a custom provider has cached `models`, Codex builds that provider's model
manager from the cached list on startup and on ordinary model-list requests.
Explicit fetches from the provider-management UI force an online refresh and
then persist the refreshed model list back to the provider entry.
