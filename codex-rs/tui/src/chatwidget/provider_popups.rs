//! Provider management popups and slash-command helpers.

use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_model_provider_info::built_in_model_providers;
use ratatui::style::Stylize;
use ratatui::text::Line;

use super::*;
use crate::app_event::ProviderFormDraft;
use crate::app_event::ProviderFormField;
use crate::app_event::ProviderFormMode;

const PROVIDERS_USAGE: &str = "Usage: /providers [add|edit|delete|use] ...";

impl ChatWidget {
    pub(crate) fn open_provider_manager(&mut self) {
        let current_provider_id = self.config.model_provider_id.clone();
        let builtin_ids = built_in_model_providers(None);
        let mut providers = self
            .config
            .model_providers
            .iter()
            .map(|(id, provider)| {
                let is_builtin = builtin_ids.contains_key(id);
                (id.clone(), provider.clone(), is_builtin)
            })
            .collect::<Vec<_>>();
        providers.sort_by(|(left_id, _, _), (right_id, _, _)| left_id.cmp(right_id));

        let mut items = vec![
            SelectionItem {
                name: "Add provider".to_string(),
                description: Some("Create a custom OpenAI-compatible provider".to_string()),
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenProviderForm {
                        mode: ProviderFormMode::Add,
                        draft: ProviderFormDraft {
                            id: "my-provider".to_string(),
                            name: "My Provider".to_string(),
                            base_url: "https://api.example.com/v1".to_string(),
                            env_key: "MY_PROVIDER_API_KEY".to_string(),
                            wire_api: WireApi::Chat,
                        },
                    });
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Refresh list".to_string(),
                description: Some("Reload provider configuration from disk".to_string()),
                actions: vec![Box::new(|tx| tx.send(AppEvent::OpenProviderManager))],
                dismiss_on_select: true,
                ..Default::default()
            },
        ];

        for (id, provider, is_builtin) in providers {
            let title = provider_title(&id, &provider);
            let description = Some(provider_description(&provider, is_builtin));
            let detail_id = id.clone();
            items.push(SelectionItem {
                name: title,
                description,
                is_current: id == current_provider_id,
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::OpenProviderDetail {
                        id: detail_id.clone(),
                    });
                })],
                dismiss_on_select: false,
                search_value: Some(format!(
                    "{} {} {} {}",
                    id,
                    provider.name,
                    provider.base_url.unwrap_or_default(),
                    provider.env_key.unwrap_or_default()
                )),
                ..Default::default()
            });
        }

        let header = providers_header(
            "Manage Providers",
            "Enter opens details. Add and edit use an interactive form.",
        );
        self.bottom_pane.show_selection_view(SelectionViewParams {
            is_searchable: true,
            search_placeholder: Some("Search providers".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header,
            ..Default::default()
        });
        self.request_redraw();
    }

    pub(crate) fn open_provider_detail(&mut self, id: &str) {
        let Some(provider) = self.config.model_providers.get(id).cloned() else {
            self.add_error_message(format!("Provider '{id}' is not configured."));
            self.open_provider_manager();
            return;
        };
        let is_builtin = built_in_model_providers(None).contains_key(id);
        let current = id == self.config.model_provider_id;

        let mut items = vec![
            SelectionItem {
                name: "Use this provider".to_string(),
                description: Some(if current {
                    "Already selected in config".to_string()
                } else {
                    "Set model_provider for new sessions".to_string()
                }),
                display_shortcut: Some(provider_shortcut('u')),
                is_current: current,
                actions: vec![Box::new({
                    let id = id.to_string();
                    move |tx| {
                        tx.send(AppEvent::ProviderConfigAction {
                            action: crate::app_event::ProviderConfigAction::Use { id: id.clone() },
                        });
                    }
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Edit provider".to_string(),
                description: Some(if is_builtin {
                    "Built-in providers cannot be edited here".to_string()
                } else {
                    "Open the provider form with current values".to_string()
                }),
                display_shortcut: Some(provider_shortcut('e')),
                is_disabled: is_builtin,
                actions: vec![Box::new({
                    let draft = provider_form_draft(id, &provider);
                    move |tx| {
                        tx.send(AppEvent::OpenProviderForm {
                            mode: ProviderFormMode::Edit,
                            draft: draft.clone(),
                        });
                    }
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Delete provider".to_string(),
                description: Some(if is_builtin {
                    "Built-in providers cannot be deleted".to_string()
                } else {
                    "Open a confirmation prompt".to_string()
                }),
                display_shortcut: Some(provider_shortcut('d')),
                is_disabled: is_builtin,
                actions: vec![Box::new({
                    let id = id.to_string();
                    move |tx| tx.send(AppEvent::OpenProviderDeleteConfirm { id: id.clone() })
                })],
                dismiss_on_select: false,
                ..Default::default()
            },
            SelectionItem {
                name: "Back to providers".to_string(),
                description: Some("Return to the provider list".to_string()),
                display_shortcut: Some(provider_shortcut('b')),
                actions: vec![Box::new(|tx| tx.send(AppEvent::OpenProviderManager))],
                dismiss_on_select: false,
                ..Default::default()
            },
        ];

        if !is_builtin {
            items.push(SelectionItem {
                name: "Add similar provider".to_string(),
                description: Some("Open the provider form using these values".to_string()),
                actions: vec![Box::new({
                    let mut cloned = provider.clone();
                    cloned.name = format!("{} Copy", provider.name);
                    let draft = provider_form_draft(&format!("{id}-copy"), &cloned);
                    move |tx| {
                        tx.send(AppEvent::OpenProviderForm {
                            mode: ProviderFormMode::Add,
                            draft: draft.clone(),
                        });
                    }
                })],
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        let header = provider_detail_header(id, &provider, is_builtin);
        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header,
            on_cancel: Some(Box::new(|tx| tx.send(AppEvent::OpenProviderManager))),
            ..Default::default()
        });
        self.request_redraw();
    }

    pub(crate) fn open_provider_delete_confirm(&mut self, id: &str) {
        let Some(provider) = self.config.model_providers.get(id) else {
            self.add_error_message(format!("Provider '{id}' is not configured."));
            self.open_provider_manager();
            return;
        };
        let title = provider_title(id, provider);
        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some(format!("Delete {title}?")),
            subtitle: Some("This removes the provider from config.toml.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            on_cancel: Some(Box::new({
                let id = id.to_string();
                move |tx| tx.send(AppEvent::OpenProviderDetail { id: id.clone() })
            })),
            items: vec![
                SelectionItem {
                    name: "Cancel".to_string(),
                    description: Some("Keep this provider".to_string()),
                    actions: vec![Box::new({
                        let id = id.to_string();
                        move |tx| tx.send(AppEvent::OpenProviderDetail { id: id.clone() })
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Delete provider".to_string(),
                    description: Some("Remove it from model_providers".to_string()),
                    actions: vec![Box::new({
                        let id = id.to_string();
                        move |tx| {
                            tx.send(AppEvent::ProviderConfigAction {
                                action: crate::app_event::ProviderConfigAction::Delete {
                                    id: id.clone(),
                                },
                            });
                        }
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        self.request_redraw();
    }

    pub(crate) fn open_provider_form(&mut self, mode: ProviderFormMode, draft: ProviderFormDraft) {
        let field = match mode {
            ProviderFormMode::Add => ProviderFormField::Id,
            ProviderFormMode::Edit => ProviderFormField::Name,
        };
        self.open_provider_form_field(mode, draft, field);
    }

    pub(crate) fn handle_provider_form_field(
        &mut self,
        mode: ProviderFormMode,
        mut draft: ProviderFormDraft,
        field: ProviderFormField,
        value: String,
    ) {
        let value = value.trim().to_string();
        match field {
            ProviderFormField::Id => draft.id = value,
            ProviderFormField::Name => draft.name = value,
            ProviderFormField::BaseUrl => draft.base_url = value,
            ProviderFormField::EnvKey => draft.env_key = value,
        }

        match next_provider_form_field(mode, field) {
            Some(next_field) => self.open_provider_form_field(mode, draft, next_field),
            None => self.open_provider_wire_api_picker(mode, draft),
        }
    }

    pub(crate) fn open_provider_form_confirm(
        &mut self,
        mode: ProviderFormMode,
        draft: ProviderFormDraft,
    ) {
        let provider = self.provider_from_form_draft(mode, &draft);
        let validation_error = self.validate_provider_form(mode, &draft, &provider);
        let action_label = match mode {
            ProviderFormMode::Add => "Confirm add provider",
            ProviderFormMode::Edit => "Confirm changes",
        };
        let action_description = match &validation_error {
            Some(err) => err.clone(),
            None => "Write this provider to config.toml".to_string(),
        };

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some(provider_form_title(mode).to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            header: provider_form_confirm_header(&draft),
            on_cancel: Some(Box::new(|tx| tx.send(AppEvent::OpenProviderManager))),
            items: vec![
                SelectionItem {
                    name: action_label.to_string(),
                    description: Some(action_description),
                    is_disabled: validation_error.is_some(),
                    actions: vec![Box::new({
                        let id = draft.id.clone();
                        let provider = provider.clone();
                        move |tx| {
                            tx.send(AppEvent::ProviderConfigAction {
                                action: crate::app_event::ProviderConfigAction::Upsert {
                                    id: id.clone(),
                                    provider: provider.clone(),
                                },
                            });
                        }
                    })],
                    dismiss_on_select: validation_error.is_none(),
                    ..Default::default()
                },
                SelectionItem {
                    name: "Edit fields".to_string(),
                    description: Some("Return to the first editable field".to_string()),
                    actions: vec![Box::new({
                        let draft = draft.clone();
                        move |tx| {
                            tx.send(AppEvent::OpenProviderForm {
                                mode,
                                draft: draft.clone(),
                            });
                        }
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Cancel".to_string(),
                    description: Some("Discard these provider changes".to_string()),
                    actions: vec![Box::new(|tx| tx.send(AppEvent::OpenProviderManager))],
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        self.request_redraw();
    }

    fn open_provider_form_field(
        &mut self,
        mode: ProviderFormMode,
        draft: ProviderFormDraft,
        field: ProviderFormField,
    ) {
        let tx = self.app_event_tx.clone();
        let title = provider_form_field_title(mode, field);
        let placeholder = provider_form_field_placeholder(field);
        let initial_text = provider_form_field_value(&draft, field);
        let context_label = Some(provider_form_context_label(mode, field, &draft));
        let view = CustomPromptView::new(
            title,
            placeholder,
            initial_text,
            context_label,
            Box::new(move |value: String| {
                tx.send(AppEvent::ProviderFormFieldSubmitted {
                    mode,
                    draft: draft.clone(),
                    field,
                    value,
                });
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
        self.request_redraw();
    }

    fn open_provider_wire_api_picker(&mut self, mode: ProviderFormMode, draft: ProviderFormDraft) {
        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Provider wire API".to_string()),
            subtitle: Some("Choose how Codex should talk to this provider.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            on_cancel: Some(Box::new(|tx| tx.send(AppEvent::OpenProviderManager))),
            items: vec![
                wire_api_item(mode, draft.clone(), WireApi::Chat),
                wire_api_item(mode, draft, WireApi::Responses),
            ],
            ..Default::default()
        });
        self.request_redraw();
    }

    fn validate_provider_form(
        &self,
        mode: ProviderFormMode,
        draft: &ProviderFormDraft,
        provider: &ModelProviderInfo,
    ) -> Option<String> {
        if draft.id.trim().is_empty() {
            return Some("Provider id cannot be empty.".to_string());
        }
        let builtin_ids = built_in_model_providers(None);
        if builtin_ids.contains_key(&draft.id) {
            return Some(format!(
                "Built-in provider '{}' cannot be edited here.",
                draft.id
            ));
        }
        match mode {
            ProviderFormMode::Add if self.config.model_providers.contains_key(&draft.id) => {
                return Some(format!("Provider '{}' already exists.", draft.id));
            }
            ProviderFormMode::Edit if !self.config.model_providers.contains_key(&draft.id) => {
                return Some(format!("Provider '{}' does not exist.", draft.id));
            }
            ProviderFormMode::Add | ProviderFormMode::Edit => {}
        }
        provider
            .validate()
            .err()
            .map(|err| format!("Invalid provider '{}': {err}", draft.id))
    }

    fn provider_from_form_draft(
        &self,
        mode: ProviderFormMode,
        draft: &ProviderFormDraft,
    ) -> ModelProviderInfo {
        let mut provider = match mode {
            ProviderFormMode::Add => ModelProviderInfo::default(),
            ProviderFormMode::Edit => self
                .config
                .model_providers
                .get(&draft.id)
                .cloned()
                .unwrap_or_default(),
        };
        provider.name = draft.name.trim().to_string();
        provider.base_url = Some(draft.base_url.trim().to_string());
        provider.env_key = (draft.env_key.trim() != "-").then(|| draft.env_key.trim().to_string());
        provider.wire_api = draft.wire_api;
        provider
    }

    pub(crate) fn handle_provider_command_args(&mut self, args: &str) {
        let Some(parts) = shlex::split(args) else {
            self.add_error_message("Could not parse provider command arguments.".to_string());
            return;
        };
        let Some((subcommand, rest)) = parts.split_first() else {
            self.open_provider_manager();
            return;
        };
        match subcommand.as_str() {
            "add" => self.handle_provider_upsert_args(rest, /*is_edit*/ false),
            "edit" => self.handle_provider_upsert_args(rest, /*is_edit*/ true),
            "delete" | "remove" => self.handle_provider_delete_args(rest),
            "use" | "select" => self.handle_provider_use_args(rest),
            _ => self.add_error_message(PROVIDERS_USAGE.to_string()),
        }
    }

    fn handle_provider_upsert_args(&mut self, args: &[String], is_edit: bool) {
        let [id, name, base_url, env_key, rest @ ..] = args else {
            self.add_error_message(
                "Usage: /providers add <id> <name> <base_url> <env_key|-> [chat|responses]"
                    .to_string(),
            );
            return;
        };
        if id.trim().is_empty() {
            self.add_error_message("Provider id cannot be empty.".to_string());
            return;
        }
        let builtin_ids = built_in_model_providers(None);
        if builtin_ids.contains_key(id) {
            self.add_error_message(format!("Built-in provider '{id}' cannot be edited here."));
            return;
        }
        let existing = self.config.model_providers.get(id).cloned();
        if is_edit && existing.is_none() {
            self.add_error_message(format!("Provider '{id}' does not exist."));
            return;
        }
        if !is_edit && existing.is_some() {
            self.add_error_message(format!(
                "Provider '{id}' already exists. Use /providers edit."
            ));
            return;
        }
        let wire_api = match rest {
            [] => WireApi::Chat,
            [wire] => match parse_wire_api(wire) {
                Some(wire_api) => wire_api,
                None => {
                    self.add_error_message("wire_api must be chat or responses.".to_string());
                    return;
                }
            },
            _ => {
                self.add_error_message(
                    "Usage: /providers edit <id> <name> <base_url> <env_key|-> [chat|responses]"
                        .to_string(),
                );
                return;
            }
        };

        let mut provider = existing.unwrap_or_default();
        provider.name = name.clone();
        provider.base_url = Some(base_url.clone());
        provider.env_key = (env_key != "-").then(|| env_key.clone());
        provider.wire_api = wire_api;
        if let Err(err) = provider.validate() {
            self.add_error_message(format!("Invalid provider '{id}': {err}"));
            return;
        }

        self.app_event_tx.send(AppEvent::ProviderConfigAction {
            action: crate::app_event::ProviderConfigAction::Upsert {
                id: id.clone(),
                provider,
            },
        });
    }

    fn handle_provider_delete_args(&mut self, args: &[String]) {
        let [id] = args else {
            self.add_error_message("Usage: /providers delete <id>".to_string());
            return;
        };
        self.app_event_tx.send(AppEvent::ProviderConfigAction {
            action: crate::app_event::ProviderConfigAction::Delete { id: id.clone() },
        });
    }

    fn handle_provider_use_args(&mut self, args: &[String]) {
        let [id] = args else {
            self.add_error_message("Usage: /providers use <id>".to_string());
            return;
        };
        self.app_event_tx.send(AppEvent::ProviderConfigAction {
            action: crate::app_event::ProviderConfigAction::Use { id: id.clone() },
        });
    }
}

fn provider_title(id: &str, provider: &ModelProviderInfo) -> String {
    if provider.name.trim().is_empty() {
        id.to_string()
    } else {
        format!("{} ({id})", provider.name)
    }
}

fn provider_description(provider: &ModelProviderInfo, is_builtin: bool) -> String {
    let source = if is_builtin { "built-in" } else { "custom" };
    let base_url = provider.base_url.as_deref().unwrap_or("no base_url");
    let env_key = provider.env_key.as_deref().unwrap_or("no env_key");
    format!("{source} - {} - {base_url} - {env_key}", provider.wire_api)
}

fn providers_header(title: &str, subtitle: &str) -> Box<dyn Renderable> {
    let mut header = ColumnRenderable::new();
    header.push(Line::from(title.to_string().bold()));
    header.push(Line::from(subtitle.to_string().dim()));
    Box::new(header)
}

fn provider_detail_header(
    id: &str,
    provider: &ModelProviderInfo,
    is_builtin: bool,
) -> Box<dyn Renderable> {
    let mut header = ColumnRenderable::new();
    header.push(Line::from(provider_title(id, provider).bold()));
    header.push(Line::from(provider_description(provider, is_builtin).dim()));
    Box::new(header)
}

fn provider_form_draft(id: &str, provider: &ModelProviderInfo) -> ProviderFormDraft {
    ProviderFormDraft {
        id: id.to_string(),
        name: provider.name.clone(),
        base_url: provider
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.example.com/v1".to_string()),
        env_key: provider.env_key.clone().unwrap_or_else(|| "-".to_string()),
        wire_api: provider.wire_api,
    }
}

fn provider_form_title(mode: ProviderFormMode) -> &'static str {
    match mode {
        ProviderFormMode::Add => "Add provider",
        ProviderFormMode::Edit => "Edit provider",
    }
}

fn provider_form_field_title(mode: ProviderFormMode, field: ProviderFormField) -> String {
    let form_title = provider_form_title(mode);
    let field_title = match field {
        ProviderFormField::Id => "Provider id",
        ProviderFormField::Name => "Display name",
        ProviderFormField::BaseUrl => "Base URL",
        ProviderFormField::EnvKey => "API key env var",
    };
    format!("{form_title}: {field_title}")
}

fn provider_form_field_placeholder(field: ProviderFormField) -> String {
    match field {
        ProviderFormField::Id => "provider-id".to_string(),
        ProviderFormField::Name => "My Provider".to_string(),
        ProviderFormField::BaseUrl => "https://api.example.com/v1".to_string(),
        ProviderFormField::EnvKey => "ENV_VAR_NAME or - for no env var".to_string(),
    }
}

fn provider_form_field_value(draft: &ProviderFormDraft, field: ProviderFormField) -> String {
    match field {
        ProviderFormField::Id => draft.id.clone(),
        ProviderFormField::Name => draft.name.clone(),
        ProviderFormField::BaseUrl => draft.base_url.clone(),
        ProviderFormField::EnvKey => draft.env_key.clone(),
    }
}

fn provider_form_context_label(
    mode: ProviderFormMode,
    field: ProviderFormField,
    draft: &ProviderFormDraft,
) -> String {
    match (mode, field) {
        (ProviderFormMode::Edit, ProviderFormField::Name) => {
            format!("Editing provider id: {}", draft.id)
        }
        (_, ProviderFormField::EnvKey) => {
            "Use - when the provider does not need an env var".to_string()
        }
        (_, ProviderFormField::BaseUrl) => {
            "Include /v1 when the provider expects OpenAI-compatible paths".to_string()
        }
        _ => "Press Enter to continue, Esc to cancel".to_string(),
    }
}

fn next_provider_form_field(
    mode: ProviderFormMode,
    field: ProviderFormField,
) -> Option<ProviderFormField> {
    match (mode, field) {
        (ProviderFormMode::Add, ProviderFormField::Id) => Some(ProviderFormField::Name),
        (_, ProviderFormField::Name) => Some(ProviderFormField::BaseUrl),
        (_, ProviderFormField::BaseUrl) => Some(ProviderFormField::EnvKey),
        (_, ProviderFormField::EnvKey) => None,
        (ProviderFormMode::Edit, ProviderFormField::Id) => Some(ProviderFormField::Name),
    }
}

fn wire_api_item(
    mode: ProviderFormMode,
    draft: ProviderFormDraft,
    wire_api: WireApi,
) -> SelectionItem {
    SelectionItem {
        name: wire_api.to_string(),
        description: Some(match wire_api {
            WireApi::Chat => "Use /v1/chat/completions".to_string(),
            WireApi::Responses => "Use /v1/responses".to_string(),
        }),
        is_current: draft.wire_api == wire_api,
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::ProviderFormWireApiSelected {
                mode,
                draft: draft.clone(),
                wire_api,
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn provider_form_confirm_header(draft: &ProviderFormDraft) -> Box<dyn Renderable> {
    let mut header = ColumnRenderable::new();
    header.push(Line::from("Review provider settings".bold()));
    header.push(Line::from(vec!["id: ".dim(), draft.id.clone().into()]));
    header.push(Line::from(vec!["name: ".dim(), draft.name.clone().into()]));
    header.push(Line::from(vec![
        "base_url: ".dim(),
        draft.base_url.clone().into(),
    ]));
    header.push(Line::from(vec![
        "env_key: ".dim(),
        draft.env_key.clone().into(),
    ]));
    header.push(Line::from(vec![
        "wire_api: ".dim(),
        draft.wire_api.to_string().into(),
    ]));
    Box::new(header)
}

fn parse_wire_api(value: &str) -> Option<WireApi> {
    match value {
        "chat" => Some(WireApi::Chat),
        "responses" => Some(WireApi::Responses),
        _ => None,
    }
}

fn provider_shortcut(ch: char) -> KeyBinding {
    KeyBinding::new(KeyCode::Char(ch), KeyModifiers::NONE)
}
