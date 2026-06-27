//! Provider selection and management popups for `ChatWidget`.
//!
//! `/provider` opens a list of all configured model providers (built-in + custom)
//! and lets the user switch between them.

use super::*;
use crate::app_event::NewProviderFormPreferences;
use crate::chatwidget::AppEventSender;

use crate::bottom_pane::BottomPaneView;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::key_hint;
use crate::keymap::ListKeymap;
use codex_model_provider_info::AMAZON_BEDROCK_PROVIDER_ID;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Block;

pub(super) const PROVIDER_SELECTION_VIEW_ID: &str = "provider-selection";

impl ChatWidget {
    /// Open the provider selection popup.
    pub(crate) fn open_provider_popup(&mut self) {
        let items = self.build_provider_selection_items();

        let header = self.provider_menu_header(
            "Select Provider",
            "Pick a model provider for this session or add a custom one.",
        );

        self.bottom_pane.show_selection_view(SelectionViewParams {
            view_id: Some(PROVIDER_SELECTION_VIEW_ID),
            footer_hint: Some(Line::from(vec![
                "Press ".into(),
                key_hint::ctrl(KeyCode::Char('e')).into(),
                " to edit · ".into(),
                key_hint::ctrl(KeyCode::Char('d')).into(),
                " to delete · ".into(),
                key_hint::plain(KeyCode::Enter).into(),
                " to confirm or ".into(),
                key_hint::plain(KeyCode::Esc).into(),
                " to go back".into(),
            ])),
            items,
            header,
            ..Default::default()
        });
    }

    fn provider_menu_header(&self, title: &str, subtitle: &str) -> Box<dyn Renderable> {
        let title = title.to_string();
        let subtitle = subtitle.to_string();
        let mut header = ColumnRenderable::new();
        header.push(Line::from(title.bold()));
        header.push(Line::from(subtitle.dim()));
        Box::new(header)
    }

    /// Open the provider form for adding or editing a custom provider.
    pub(crate) fn open_provider_form(&mut self, prefill: Option<NewProviderFormPreferences>) {
        let view = ProviderFormView::new(
            prefill,
            self.app_event_tx.clone(),
            self.bottom_pane.list_keymap(),
        );
        self.bottom_pane.show_view(Box::new(view));
    }

    /// Build all selection items for the provider list.
    fn build_provider_selection_items(&mut self) -> Vec<SelectionItem> {
        let current_provider_id = self.config.model_provider_id.as_str();
        let mut items: Vec<SelectionItem> = Vec::new();

        // Collect providers sorted by key for deterministic ordering
        let mut providers: Vec<(&String, &ModelProviderInfo)> = self
            .config
            .model_providers
            .iter()
            .filter(|(key, _)| *key != AMAZON_BEDROCK_PROVIDER_ID)
            .collect();
        providers.sort_by_key(|(key, _)| *key);

        for (provider_id, provider) in providers {
            let display_name = if provider.name.trim().is_empty() {
                provider_id.clone()
            } else {
                provider.name.trim().to_string()
            };

            let description = build_provider_description(provider, provider_id);

            let is_current = provider_id.as_str() == current_provider_id;
            let select_id = provider_id.clone();
            let actions: Vec<_> =
                vec![
                    Box::new(move |tx: &crate::app_event_sender::AppEventSender| {
                        tx.send(AppEvent::UpdateModelProvider(
                            select_id.clone(),
                        ));
                        tx.send(AppEvent::PersistProviderSelection {
                            provider_id: select_id.clone(),
                        });
                    })
                        as Box<dyn Fn(&crate::app_event_sender::AppEventSender) + Send + Sync>,
                ];

            items.push(SelectionItem {
                name: display_name,
                description: Some(description),
                is_current,
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        // Add the "Add Custom Provider..." entry
        let add_actions: Vec<_> =
            vec![
                Box::new(move |tx: &crate::app_event_sender::AppEventSender| {
                    tx.send(AppEvent::OpenProviderForm {
                        prefill_provider: None,
                    });
                })
                    as Box<dyn Fn(&crate::app_event_sender::AppEventSender) + Send + Sync>,
            ];
        items.push(SelectionItem {
            name: "+ Add Custom Provider".to_string(),
            description: Some(
                "Configure a new third-party provider with a custom base URL and API key"
                    .to_string(),
            ),
            is_current: false,
            actions: add_actions,
            dismiss_on_select: false,
            ..Default::default()
        });

        items
    }

    /// Handle keyboard shortcuts for the provider selection popup.
    /// Returns `true` if the event was handled by this popup.
    pub(crate) fn handle_provider_popup_key_event(&mut self, key_event: KeyEvent) -> bool {
        let edit = key_hint::ctrl(KeyCode::Char('e')).is_press(key_event);
        let delete = key_hint::ctrl(KeyCode::Char('d')).is_press(key_event);
        if !edit && !delete {
            return false;
        }

        // Only fire when the provider popup is the active view.
        let Some(selected_index) = self
            .bottom_pane
            .selected_index_for_active_view(PROVIDER_SELECTION_VIEW_ID)
        else {
            return false;
        };

        // Rebuild the provider ID list in the same order as build_provider_selection_items
        // to map the selected index back to a provider_id.
        let provider_ids: Vec<String> = {
            let mut ids: Vec<&String> = self
                .config
                .model_providers
                .iter()
                .filter(|(key, _)| *key != AMAZON_BEDROCK_PROVIDER_ID)
                .map(|(key, _)| key)
                .collect();
            ids.sort_by_key(|key| (*key).clone());
            ids.into_iter().cloned().collect()
        };

        let Some(provider_id) = provider_ids.get(selected_index) else {
            return false;
        };

        let provider_id = provider_id.clone();

        if edit {
            self.app_event_tx.send(AppEvent::EditProviderForm {
                provider_id,
            });
        } else {
            self.app_event_tx.send(AppEvent::DeleteProvider {
                provider_id,
            });
        }

        true
    }

}

/// Build a human-readable description line for a provider.
fn build_provider_description(provider: &ModelProviderInfo, provider_id: &str) -> String {
    let wire_api_label = match provider.wire_api {
        WireApi::Responses => "Responses API",
        WireApi::ChatCompletions => "Chat Completions",
    };

    let mut parts = vec![wire_api_label.to_string()];

    if let Some(ref base_url) = provider.base_url {
        let is_default_openai = matches!(provider.wire_api, WireApi::Responses)
            && (base_url.trim() == "https://api.openai.com/v1" || base_url.trim().is_empty());
        if !is_default_openai || provider_id != "openai" {
            let sanitized = sanitize_base_url(base_url);
            parts.push(sanitized);
        }
    }

    parts.join(" • ")
}

/// Strip credentials and trailing cruft from a base URL for safe display.
fn sanitize_base_url(url: &str) -> String {
    let url = url.trim_end_matches('/');
    url.to_string()
}

// ---------------------------------------------------------------------------
// ProviderFormView — a bottom-pane form for adding a new model provider.
// ---------------------------------------------------------------------------

/// Fields in the provider form that the user can navigate between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormField {
    Name,
    BaseUrl,
    WireApi,
    ApiKey,
    EnvKey,
    Save,
    Cancel,
}

impl FormField {
    fn all() -> [Self; 7] {
        [
            Self::Name,
            Self::BaseUrl,
            Self::WireApi,
            Self::ApiKey,
            Self::EnvKey,
            Self::Save,
            Self::Cancel,
        ]
    }

    fn next(self) -> Self {
        let fields = Self::all();
        let idx = fields.iter().position(|f| *f == self).unwrap_or(0);
        fields[(idx + 1) % fields.len()]
    }

    fn prev(self) -> Self {
        let fields = Self::all();
        let idx = fields.iter().position(|f| *f == self).unwrap_or(0);
        fields[(idx + fields.len() - 1) % fields.len()]
    }

    fn is_text_field(self) -> bool {
        matches!(
            self,
            Self::Name | Self::BaseUrl | Self::ApiKey | Self::EnvKey
        )
    }
}

/// Holds the current values of each form field.
#[derive(Debug, Clone)]
struct ProviderFormData {
    name: String,
    base_url: String,
    wire_api: WireApi,
    api_key: String,
    env_key: String,
}

impl ProviderFormData {
    fn new(prefill: Option<NewProviderFormPreferences>) -> Self {
        match prefill {
            Some(p) => Self {
                name: p.name,
                base_url: p.base_url,
                wire_api: p.wire_api,
                api_key: p.api_key,
                env_key: p.env_key,
            },
            None => Self {
                name: String::new(),
                base_url: String::new(),
                wire_api: WireApi::Responses,
                api_key: String::new(),
                env_key: String::new(),
            },
        }
    }
}

pub(crate) struct ProviderFormView {
    form_data: ProviderFormData,
    focused_field: FormField,
    complete: bool,
    app_event_tx: AppEventSender,
    keymap: ListKeymap,
    provider_id: String,
}

impl ProviderFormView {
    pub(crate) fn new(
        prefill: Option<NewProviderFormPreferences>,
        app_event_tx: AppEventSender,
        keymap: ListKeymap,
    ) -> Self {
        let provider_id = prefill.as_ref().map(|p| p.provider_id.clone()).unwrap_or_default();

        Self {
            form_data: ProviderFormData::new(prefill),
            focused_field: FormField::Name,
            complete: false,
            app_event_tx,
            keymap,
            provider_id,
        }
    }

    fn label_for_field(field: FormField) -> &'static str {
        match field {
            FormField::Name => "Name",
            FormField::BaseUrl => "Base URL",
            FormField::WireApi => "API Type",
            FormField::ApiKey => "API Key",
            FormField::EnvKey => "Environment Variable",
            FormField::Save => "",
            FormField::Cancel => "",
        }
    }

    fn value_for_field(&self, field: FormField) -> String {
        match field {
            FormField::Name => self.form_data.name.clone(),
            FormField::BaseUrl => self.form_data.base_url.clone(),
            FormField::WireApi => match self.form_data.wire_api {
                WireApi::Responses => "Responses API".to_string(),
                WireApi::ChatCompletions => "Chat Completions".to_string(),
            },
            FormField::ApiKey => self.form_data.api_key.clone(),
            FormField::EnvKey => self.form_data.env_key.clone(),
            FormField::Save => String::new(),
            FormField::Cancel => String::new(),
        }
    }

    fn edit_field(&mut self, field: FormField, ch: char) {
        match field {
            FormField::ApiKey => {
                self.form_data.api_key.push(ch);
                self.form_data.env_key.clear();
            }
            FormField::EnvKey => {
                self.form_data.env_key.push(ch);
                self.form_data.api_key.clear();
            }
            FormField::Name => self.form_data.name.push(ch),
            FormField::BaseUrl => self.form_data.base_url.push(ch),
            FormField::WireApi => {} // handled separately
            FormField::Save | FormField::Cancel => {}
        }
    }

    fn backspace_field(&mut self, field: FormField) {
        match field {
            FormField::Name => _ = self.form_data.name.pop(),
            FormField::BaseUrl => _ = self.form_data.base_url.pop(),
            FormField::ApiKey => {
                _ = self.form_data.api_key.pop();
            }
            FormField::EnvKey => {
                _ = self.form_data.env_key.pop();
            }
            FormField::WireApi | FormField::Save | FormField::Cancel => {}
        }
    }

    fn toggle_wire_api(&mut self) {
        self.form_data.wire_api = match self.form_data.wire_api {
            WireApi::Responses => WireApi::ChatCompletions,
            WireApi::ChatCompletions => WireApi::Responses,
        };
    }

    fn save_and_dismiss(&mut self) {
        let provider_id = if self.provider_id.is_empty() {
            // Sanitize: the provider name becomes a JSON key in model_providers.{key}.
            // Replace whitespace and dots with underscores, strip other unsafe chars
            // so the resulting key works as a config TOML/JSON path segment.
            let sanitized: String = self
                .form_data
                .name
                .chars()
                .map(|c| match c {
                    '.' | ' ' | '\t' | '\n' => '_',
                    c if c.is_alphanumeric() || c == '_' || c == '-' => c,
                    _ => '_',
                })
                .filter(|&c| c != '\0')
                .collect();
            if sanitized.is_empty() {
                // fallback: use a placeholder to avoid an empty key
                "custom_provider".to_string()
            } else {
                sanitized
            }
        } else {
            self.provider_id.clone()
        };
        let pid = provider_id.clone();
        self.app_event_tx.send(AppEvent::SaveNewProvider {
            provider_id,
            name: self.form_data.name.clone(),
            base_url: self.form_data.base_url.clone(),
            wire_api: self.form_data.wire_api,
            api_key: self.form_data.api_key.clone(),
            env_key: self.form_data.env_key.clone(),
        });
        // Also emit ProviderFormSaved so event_dispatch switches the active provider
        // and reports the change to the user.
        self.app_event_tx.send(AppEvent::ProviderFormSaved {
            provider_id: pid,
        });
        self.complete = true;
    }

    fn cancel_and_dismiss(&mut self) {
        self.app_event_tx.send(AppEvent::ProviderFormDismissed);
        self.complete = true;
    }

    fn build_form_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        for field in FormField::all() {
            match field {
                f @ FormField::Save => {
                    let save_label = if self.focused_field == f {
                        "[ Save ]".to_string()
                    } else {
                        "  Save  ".to_string()
                    };
                    let cancel_label = if self.focused_field == FormField::Cancel {
                        "[ Cancel ]".to_string()
                    } else {
                        "  Cancel  ".to_string()
                    };
                    let label = format!("{save_label}    {cancel_label}");
                    if self.focused_field == f || self.focused_field == FormField::Cancel {
                        lines.push(Line::from(label.bold()));
                    } else {
                        lines.push(Line::from(label));
                    }
                }
                f if f.is_text_field() || f == FormField::WireApi => {
                    let label = Self::label_for_field(f);
                    let value = self.value_for_field(f);
                    let indicator = if self.focused_field == f { '>' } else { ' ' };
                    let display = if f == FormField::WireApi {
                        format!("{indicator} {label}: {value}")
                    } else {
                        // Always mask API key values as they are secrets.
                        // Env Key stores a variable name (not the secret itself), so it can be shown.
                        let masked = if f == FormField::ApiKey && !value.is_empty() {
                            "********".to_string()
                        } else {
                            value.clone()
                        };
                        format!("{indicator} {label}: {masked}")
                    };
                    // Dim the mutually exclusive field (ApiKey <-> EnvKey) when the other is focused.
                    let is_api_or_env = f == FormField::ApiKey || f == FormField::EnvKey;
                    let is_dimmed = (f == FormField::ApiKey
                        && self.focused_field == FormField::EnvKey
                        && !self.form_data.env_key.is_empty())
                        || (f == FormField::EnvKey
                            && self.focused_field == FormField::ApiKey
                            && !self.form_data.api_key.is_empty());
                    if self.focused_field == f {
                        lines.push(Line::from(display.bold()));
                    } else if is_dimmed {
                        lines.push(Line::from(format!("{display}  (clear the other to switch)").dim()));
                    } else if is_api_or_env && value.is_empty() {
                        let hint = if f == FormField::ApiKey {
                            " (or use Environment Variable below)"
                        } else {
                            " (or set API Key above)"
                        };
                        lines.push(Line::from(format!("{display}{hint}")));
                    } else {
                        lines.push(Line::from(display));
                    }
                }
                _ => {}
            }
        }

        lines
    }
}

impl BottomPaneView for ProviderFormView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            _ if self.keymap.accept.is_pressed(key_event)
                && self.focused_field == FormField::Save =>
            {
                self.save_and_dismiss();
            }
            _ if self.keymap.cancel.is_pressed(key_event) => {
                self.cancel_and_dismiss();
            }
            KeyEvent {
                code: KeyCode::Tab,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.focused_field = self.focused_field.next();
            }
            KeyEvent {
                code: KeyCode::Tab,
                modifiers: KeyModifiers::SHIFT,
                ..
            }
            | KeyEvent {
                code: KeyCode::Up,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.focused_field = self.focused_field.prev();
            }
            KeyEvent {
                code: KeyCode::Down,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.focused_field = self.focused_field.next();
            }
            KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.focused_field = self.focused_field.prev();
            }
            KeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.focused_field = self.focused_field.next();
            }
            KeyEvent {
                code: KeyCode::Char(' '),
                modifiers: KeyModifiers::NONE,
                ..
            } if self.focused_field == FormField::WireApi => {
                self.toggle_wire_api();
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } if self.focused_field == FormField::WireApi => {
                self.toggle_wire_api();
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } if self.focused_field == FormField::Save => {
                self.save_and_dismiss();
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } if self.focused_field == FormField::Cancel => {
                self.cancel_and_dismiss();
            }
            KeyEvent {
                code: KeyCode::Char(ch),
                modifiers: KeyModifiers::NONE,
                ..
            } if self.focused_field.is_text_field() => {
                self.edit_field(self.focused_field, ch);
            }
            KeyEvent {
                code: KeyCode::Backspace,
                modifiers: KeyModifiers::NONE,
                ..
            } if self.focused_field.is_text_field() => {
                self.backspace_field(self.focused_field);
            }
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn on_ctrl_c(&mut self) -> crate::chatwidget::CancellationEvent {
        self.cancel_and_dismiss();
        crate::chatwidget::CancellationEvent::Handled
    }
}

impl Renderable for ProviderFormView {
    fn desired_height(&self, _width: u16) -> u16 {
        // A header + each field + some spacing
        let field_count = FormField::all().len(); // 7
        field_count as u16 + 2
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let [content_area, footer_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);

        Block::default()
            .style(crate::style::user_message_style())
            .render(content_area, buf);

        let lines = self.build_form_lines(content_area.width.saturating_sub(4));
        let y_offset = 1;

        for (i, line) in lines.iter().enumerate() {
            if i + y_offset >= content_area.height as usize {
                break;
            }
            let line_area = Rect {
                x: content_area.x + 2,
                y: content_area.y + y_offset as u16 + i as u16,
                width: content_area.width.saturating_sub(4),
                height: 1,
            };
            line.clone().render(line_area, buf);
        }

        // Footer hint
        let hint =
            "Tab/Shift+Tab or ↑/↓ to navigate · Enter/Space to toggle · Ctrl+C or Esc to cancel";
        let hint_area = Rect {
            x: footer_area.x + 2,
            y: footer_area.y,
            width: footer_area.width.saturating_sub(2),
            height: footer_area.height,
        };
        Line::from(hint).dim().render(hint_area, buf);
    }
}
