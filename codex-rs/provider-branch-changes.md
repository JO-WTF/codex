# Provider branch changes

This note documents the code changes in this branch that add custom model
provider support, provider management in the TUI, Chat Completions transport
support, and provider/model-list decoupling. It is intended as an engineering
review aid, not as user-facing product documentation.

## Rebase Strategy

Keep the provider feature as a set of small layer-owned seams when rebasing:

- Preserve upstream toolchain/build configuration unless the provider feature directly requires a change; local compiler workarounds create unnecessary conflicts.
- Keep Chat Completions support isolated in `codex-api` translation/SSE modules and route to it from `core/src/client.rs` only at the `WireApi` decision point.
- Keep provider-management UI in the dedicated `chatwidget/provider_popups.rs` module instead of adding more orchestration to `chatwidget.rs`; update only the event bridge when upstream TUI event plumbing changes.
- Keep cached custom-provider models stored on `ModelProviderInfo.models`; do not merge them into the built-in OpenAI catalog.


## Branch Review Decisions

This section records the branch-level review from upstream commit `f959e7f`
through the provider branch head. The goal is to keep future rebases mechanical:
when upstream changes one layer, reapply only the matching seam below.

### Changed during review

- **Provider model cache isolation.** Custom providers now use a
  provider-namespaced model cache instead of sharing the default
  `models_cache.json`. This avoids a rebase-hostile hidden coupling where a
  fresh cache from provider A could satisfy provider B before B's `/models`
  endpoint was queried. The change belongs in `models-manager` because cache
  policy is owned there, while provider identity is supplied through the
  `ModelsEndpointClient` seam. Built-in OpenAI providers keep the legacy cache
  file name for compatibility.
- **Redundant ETag read cleanup.** `refresh_if_new_etag` no longer reads the
  current ETag twice. The second read was harmless but increased diff noise and
  made future conflict resolution less obvious.

### Reviewed and intentionally left unchanged

- **Chat Completions translation remains lossy at the API boundary.** Dropping
  Responses-only concepts such as encrypted reasoning, stateful
  `previous_response_id`, and hosted OpenAI tools is intentional. Keeping that
  loss localized in `codex-api/src/chat_translate.rs` prevents plumbing
  chat-specific conditionals through core turn execution.
- **Core request routing stays as a single `WireApi` branch.** The branch adds
  Chat Completions support without changing the Responses/WebSocket paths for
  native providers. This is the smallest durable seam: future upstream changes
  to Responses transport should usually apply on the Responses arm without
  touching chat translation.
- **App-server `model/list` keeps `forceRefresh` as the opt-in refresh knob.**
  This avoids making every model picker open perform a network call, while still
  giving provider setup and explicit fetch flows a way to bypass cached provider
  models.
- **Provider management UI remains isolated in `chatwidget/provider_popups.rs`.**
  The file is large but still below the repository's hard 800-LoC guidance for
  complex modules, and extracting smaller one-use helper modules would increase
  rebase surface rather than reduce it. The main `chatwidget.rs` only imports
  the module and remains orchestration-focused.
- **Onboarding keeps provider setup in the existing auth screen.** The first-run
  flow is already the owner of credential decisions, so adding a custom-provider
  branch there avoids a separate startup state machine. Future improvements
  should extract only if multiple provider setup screens start sharing logic.
- **Config serialization strips null provider fields.** This keeps generated
  `config.toml` edits minimal and avoids writing noisy clears into user config,
  which is important when rebasing across upstream config-shape changes.
- **Schema and TypeScript fixture updates stay checked in.** The branch changes
  app-server API shape (`ModelListParams.forceRefresh`), so generated protocol
  fixtures are part of the intentional patch rather than incidental churn.

### Follow-up risks to re-check after future rebases

- Re-run the app-server schema generator if upstream changes any v2 model-list
  payloads or config RPC conventions.
- Re-run TUI snapshot tests whenever provider popup text or model-picker
  behavior changes; these are user-visible UI surfaces.
- Re-check Chat Completions SSE parsing if upstream adds new `ResponseEvent`
  variants required by tool-call lifecycle accounting.

## Behavior Summary

The branch introduces a custom provider flow across three layers:

- Configuration can store provider-specific transport settings, including
  whether the provider uses the Responses API shape or Chat Completions shape.
- The TUI exposes provider add/edit/delete/select flows, including onboarding
  access for first-run users who do not yet have OpenAI credentials.
- The app-server and model manager no longer assume that the model list always
  belongs to the provider captured at process startup. When a provider is
  selected or edited, the TUI refreshes the model catalog from app-server, and
  app-server resolves model listing from the latest provider configuration.

The branch also disables the previous eager MCP startup connection behavior so
startup does not immediately connect to configured MCP servers.

## Chat Completions API Support

### `codex-rs/codex-api/src/chat_translate.rs`

Purpose: provide the translation layer needed when a provider exposes a
Chat Completions-compatible endpoint instead of Responses-compatible endpoint.

Changes:

- Adds conversion between Codex internal response/input structures and Chat
  Completions messages.
- Handles tool/function-call shaped data so existing Codex turns can be sent to
  a chat-style provider.
- Adds conversion safeguards for fields that are supported by Responses but do
  not have a direct Chat Completions equivalent.

### `codex-rs/codex-api/src/chat_translate_tests.rs`

Purpose: cover the new Responses-to-Chat translation behavior.

Changes:

- Adds tests for message conversion.
- Adds tests around tool/function-call output conversion.
- Verifies that the translation preserves the fields Codex needs to continue a
  turn against a chat-style provider.

### `codex-rs/codex-api/src/endpoint/chat.rs`

Purpose: define the endpoint implementation for Chat Completions-compatible
providers.

Changes:

- Adds a provider endpoint that targets the chat completions route.
- Wires request construction through the new chat translation layer.
- Provides the response handling entry point used by the API client when a
  provider is configured with chat wire format.

### `codex-rs/codex-api/src/endpoint/mod.rs`

Purpose: expose the new chat endpoint module to the rest of `codex-api`.

Changes:

- Registers the chat endpoint module.
- Allows endpoint selection code to choose between existing Responses transport
  and the new Chat Completions transport.

### `codex-rs/codex-api/src/lib.rs`

Purpose: make the chat translation and endpoint pieces available outside the
crate.

Changes:

- Exports the new chat-related modules so core request code can route provider
  calls through them.

### `codex-rs/codex-api/src/sse/chat.rs`

Purpose: parse streaming Chat Completions responses into Codex's existing event
shape.

Changes:

- Adds SSE parsing for chat delta events.
- Converts streamed assistant text and tool-call deltas into the internal stream
  items consumed by the rest of Codex.

### `codex-rs/codex-api/src/sse/mod.rs`

Purpose: expose the chat SSE parser.

Changes:

- Registers the chat streaming module next to the existing streaming parser
  implementations.

## Config And Protocol Support

### `codex-rs/config/src/thread_config/proto/codex.thread_config.v1.proto`

Purpose: carry provider wire API selection through persisted or remote thread
configuration.

Changes:

- Adds a field for provider wire API selection so thread config can distinguish
  Responses-compatible providers from Chat Completions-compatible providers.

### `codex-rs/config/src/thread_config/proto/codex.thread_config.v1.rs`

Purpose: generated Rust representation for the proto change.

Changes:

- Adds the generated Rust field corresponding to the new proto wire API field.
- Keeps Rust config serialization in sync with the proto contract.

### `codex-rs/config/src/thread_config/remote.rs`

Purpose: map remote thread config into local config with provider wire API
preserved.

Changes:

- Reads and writes the new wire API value when converting thread config.
- Ensures remote thread configuration does not lose the provider transport
  choice.

### `codex-rs/core/config.schema.json`

Purpose: keep the generated config schema aligned with the new provider config
field.

Changes:

- Adds schema coverage for the provider wire API setting.
- Allows config validation and editor tooling to understand the new field.

### `codex-rs/model-provider-info/src/lib.rs`

Purpose: extend provider metadata with the API wire format selection used by
core and TUI.

Changes:

- Adds provider metadata for whether a provider uses Responses or Chat
  Completions wire format.
- Makes built-in and custom provider definitions expose the field consistently.

## Core Request Routing

### `codex-rs/core/src/client.rs`

Purpose: route outbound model requests according to the selected provider's wire
format.

Changes:

- Adds branching for Responses-compatible versus Chat Completions-compatible
  providers.
- Uses the new chat endpoint and chat SSE parser when `wire_api` is configured
  for chat.
- Preserves the existing Responses path for official Codex/OpenAI providers and
  other providers that stay on the Responses wire format.

### `codex-rs/core/src/session/session.rs`

Purpose: adjust session startup and runtime behavior around provider state and
MCP startup side effects.

Changes:

- Stops eagerly connecting MCP servers during program startup.
- Keeps MCP state refresh deferred until the runtime actually needs that
  information.
- Ensures session setup still carries the selected model/provider values into
  turn execution.

### `codex-rs/core/tests/suite/client.rs`

Purpose: cover the new client routing behavior.

Changes:

- Adds tests for chat-wire provider request routing.
- Verifies that the request body sent to a chat-compatible provider is converted
  from Codex's existing internal turn shape.
- Protects the existing Responses path from regressions.

## Provider Model Discovery And Decoupling

### `codex-rs/models-manager/src/manager.rs`

Purpose: let non-official providers opt into remote model discovery without
requiring Codex backend auth.

Changes:

- Adds `supports_remote_model_refresh()` to `ModelsEndpointClient`.
- Keeps the default behavior conservative by delegating to command-auth support.
- Updates `should_refresh_models()` so refresh can happen either with Codex
  backend auth or when the endpoint explicitly supports remote model refresh.

### `codex-rs/model-provider/src/models_endpoint.rs`

Purpose: enable model discovery for custom OpenAI-compatible providers.

Changes:

- Implements `supports_remote_model_refresh()` for the OpenAI-compatible models
  endpoint.
- Allows custom/non-OpenAI providers to call their own `/models` endpoint.
- Keeps OpenAI-auth-required providers on the existing auth-gated behavior unless
  command auth is available.

### `codex-rs/app-server/src/models.rs`

Purpose: decouple app-server model listing from `ThreadManager`.

Changes:

- Changes `supported_models()` to accept a `SharedModelsManager` directly.
- Adds an explicit `RefreshStrategy` parameter.
- Keeps filtering and API model conversion in this module while allowing callers
  to decide which provider manager and refresh policy to use.

### `codex-rs/app-server/src/request_processors/catalog_processor.rs`

Purpose: make `model/list` use the latest provider configuration instead of the
provider captured only at app-server startup.

Changes:

- Changes `model_list()` from a static helper call into an instance method so it
  can access config manager and auth manager state.
- Reloads the latest config before serving `model/list`.
- Reuses the startup models manager only when the latest provider config still
  matches startup config.
- Builds a fresh provider models manager when the user has switched providers at
  runtime.
- Uses `Online` refresh for switched custom providers so stale global model cache
  does not hide the newly selected provider's models.
- Uses `OnlineIfUncached` for official/OpenAI-auth providers so the existing
  cache behavior is preserved.

### `codex-rs/app-server/tests/suite/v2/model_list.rs`

Purpose: prove the provider model-list path works through the public app-server
API.

Changes:

- Adds a custom provider config fixture with `wire_api = "chat"`.
- Mocks the provider's `/v1/models` endpoint and verifies the provider key is
  used.
- Asserts that `model/list` returns the remote provider model.
- Keeps existing pagination, hidden-model, cache, and ChatGPT remote-catalog
  tests intact.

## TUI Provider Management

### `codex-rs/tui/src/app_event.rs`

Purpose: add app events for provider management interactions.

Changes:

- Adds events to open the provider manager, provider detail, delete confirmation,
  and provider form.
- Adds form submission events for individual fields and wire API selection.
- Adds provider config action events for add/edit/delete/select operations.

### `codex-rs/tui/src/chatwidget/provider_popups.rs`

Purpose: implement the TUI provider management screens.

Changes:

- Adds the provider list/manager view.
- Adds provider detail view.
- Adds provider add/edit form UI.
- Adds delete confirmation UI.
- Emits app events instead of writing config directly from the widget, keeping
  persistence in the app event layer.
- Provides fields for provider id, name, base URL, auth env key, wire API, and
  default model.

### `codex-rs/tui/src/chatwidget.rs`

Purpose: register the provider popup module with the chat widget.

Changes:

- Adds the provider popup module to `ChatWidget`.
- Exposes the provider UI entry points used by slash commands and app events.

### `codex-rs/tui/src/chatwidget/slash_dispatch.rs`

Purpose: route slash commands into provider UI.

Changes:

- Adds dispatch for the provider management slash command.
- Opens the provider manager from the chat widget when the command is invoked.

### `codex-rs/tui/src/slash_command.rs`

Purpose: expose provider management in the slash command catalog.

Changes:

- Adds the provider command metadata.
- Makes provider management discoverable from the command popup.

### `codex-rs/tui/src/bottom_pane/snapshots/codex_tui__bottom_pane__command_popup__tests__command_popup_default_items.snap`

Purpose: update the snapshot for the slash command popup.

Changes:

- Adds the new provider command to the expected command list snapshot.

### `codex-rs/tui/src/app/event_dispatch.rs`

Purpose: connect provider UI events to config persistence and model-list refresh.

Changes:

- Handles events for opening provider manager/detail/form/delete confirmation.
- Handles provider form field submission and wire API selection.
- Handles provider config actions by writing config through app-server
  `config/batchWrite`.
- Refreshes in-memory TUI config after provider changes so the UI reflects the
  latest config file.
- Refreshes the TUI model catalog after selecting a provider or editing the
  currently selected provider.
- Prevents editing or deleting built-in providers.
- Prevents deleting the currently selected provider until another provider is
  selected.

### `codex-rs/tui/src/app_server_session.rs`

Purpose: expose a TUI-side helper for refreshing model catalog data.

Changes:

- Adds `fetch_available_models()`.
- Sends app-server `model/list` with hidden models included, matching bootstrap
  behavior.
- Converts app-server `Model` values back to TUI `ModelPreset` values.
- Stores the refreshed model list on the app-server session facade.

### `codex-rs/tui/src/chatwidget/settings.rs`

Purpose: allow the app layer to replace the chat widget's model catalog after a
provider switch.

Changes:

- Adds `set_model_catalog()`.
- Keeps the field itself private while allowing the app event layer to update the
  active widget through an explicit method.

### `codex-rs/tui/src/config_update.rs`

Purpose: centralize provider config writes.

Changes:

- Adds helpers to build config edits for provider add/edit.
- Adds helpers to build config edits for provider deletion.
- Adds helpers to build config edits for selecting the active provider.
- Keeps TUI UI code from hand-assembling config write payloads.

### `codex-rs/tui/src/lib.rs`

Purpose: register the new provider UI module.

Changes:

- Adds module wiring needed for provider popup code to compile and be reachable
  from the TUI.

## Onboarding Provider Entry

### `codex-rs/tui/src/onboarding/auth.rs`

Purpose: allow first-run users to configure a custom provider before signing in
  with OpenAI credentials.

Changes:

- Adds a custom provider option to the first-run auth screen.
- Implements a provider setup flow from onboarding.
- Collects provider id, display name, base URL, env key, wire API, and default
  model.
- Writes the provider config and selects it as the active provider.
- Reloads config after provider setup so the app can continue with the new
  provider.
- Improves config write error display for provider setup failures.

### `codex-rs/tui/src/onboarding/keys.rs`

Purpose: add the key/input plumbing needed by the onboarding provider flow.

Changes:

- Extends onboarding key handling so the custom provider path can accept and
  submit form values.

### `codex-rs/tui/src/onboarding/onboarding_screen.rs`

Purpose: render the onboarding entry for custom provider setup.

Changes:

- Adds the custom provider option to the first-run onboarding screen.
- Keeps the existing ChatGPT/API-key options available.
- Routes selection into the provider setup flow.

## Runtime Provider Selection Semantics

### `codex-rs/model-provider-info/src/lib.rs`

Purpose: make provider metadata complete enough for both runtime routing and TUI
forms.

Changes:

- Adds the wire API field to provider info.
- Ensures built-in provider definitions have explicit wire API behavior.
- Lets custom provider config round-trip the field used by core request routing.

### `codex-rs/core/config.schema.json`

Purpose: keep config validation in sync with the new provider metadata.

Changes:

- Adds the wire API enum/schema entry for provider config.
- Makes config files that include `wire_api` validate cleanly.

## Important Interaction Details

### Adding a provider

- The user opens the provider manager from slash command or onboarding.
- The TUI provider form collects provider metadata and emits a
  `ProviderConfigAction::Upsert`.
- The app event layer writes provider config through app-server config APIs.
- If the provider is not selected yet, the current model list is not changed.

### Selecting a provider

- The provider manager emits `ProviderConfigAction::Use`.
- The app event layer writes `model_provider`.
- The app reloads config from disk.
- The app asks app-server for `model/list`.
- App-server reloads latest config, builds the correct provider models manager,
  and returns models for the selected provider.
- The TUI replaces its in-memory `ModelCatalog`, so `/model` reflects the
  selected provider without restart.

### Editing a provider

- Editing uses the same provider form as adding.
- The app writes the updated provider config.
- If the edited provider is currently selected, the TUI refreshes the model
  catalog after the write.
- If the edited provider is not selected, the current model list is left alone.

### Deleting a provider

- Built-in providers cannot be deleted.
- The currently selected provider cannot be deleted until the user selects a
  different provider.
- Deletion removes the custom provider config entry and leaves the active model
  catalog unchanged.

### Switching back to official Codex/OpenAI

- Selecting an official provider writes `model_provider` back to that provider.
- Subsequent `model/list` calls build or reuse a models manager for the latest
  official provider config.
- Official provider refresh keeps the existing cache-oriented behavior instead
  of forcing custom-provider online refresh.

## Verification Performed

- `just fmt`
- `just test -p codex-app-server model_list`
- `just test -p codex-tui`
- `just test -p codex-models-manager`
- `just test -p codex-model-provider`
- `CODEX_HOME=/Users/zhaoyu/Projects/codex/.codex-provider-test-home cargo build -p codex-cli --bin codex`

`just fix` was attempted, but the local `1.95.0-aarch64-apple-darwin`
toolchain does not provide an applicable `cargo-clippy` binary, so Clippy fix
could not run on this machine.
