//! Authentication step UI and state transitions used by onboarding.
//!
//! This module owns the auth-step state machine (ChatGPT login/device-code/API
//! key), renders the corresponding UI, and handles auth-scoped keyboard input.
//! It intentionally does not decide onboarding flow completion; the enclosing
//! onboarding screen coordinates step progression.

#![allow(clippy::unwrap_used)]

use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_protocol::AccountLoginCompletedNotification;
use codex_app_server_protocol::AccountUpdatedNotification;
use codex_app_server_protocol::AuthMode as ApiAuthMode;
use codex_app_server_protocol::CancelLoginAccountParams;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::LoginAccountParams;
use codex_app_server_protocol::LoginAccountResponse;
use codex_login::read_openai_api_key_from_env;
use codex_model_provider::fetch_provider_models as fetch_remote_provider_models;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::ProviderModelInfo;
use codex_model_provider_info::WireApi;
use codex_model_provider_info::built_in_model_providers;
use codex_protocol::auth::AuthMode;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;

use codex_protocol::config_types::ForcedLoginMethod;
use std::cell::Cell;
use std::sync::Arc;
use std::sync::RwLock;
use uuid::Uuid;

use crate::LoginStatus;
use crate::config_update;
use crate::key_hint::KeyBinding;
use crate::key_hint::KeyBindingListExt;
use crate::motion::MotionMode;
use crate::motion::shimmer_text;
use crate::onboarding::keys;
use crate::onboarding::onboarding_screen::KeyboardHandler;
use crate::onboarding::onboarding_screen::StepStateProvider;
use crate::tui::FrameRequester;

/// Marks buffer cells that have cyan+underlined style as an OSC 8 hyperlink.
///
/// Terminal emulators recognise the OSC 8 escape sequence and treat the entire
/// marked region as a single clickable link, regardless of row wrapping.  This
/// is necessary because ratatui's cell-based rendering emits `MoveTo` at every
/// row boundary, which breaks normal terminal URL detection for long URLs that
/// wrap across multiple rows.
pub(crate) fn mark_url_hyperlink(buf: &mut Buffer, area: Rect, url: &str) {
    crate::terminal_hyperlinks::mark_url_hyperlink(buf, area, url);
}

/// Marks any underlined buffer cells as an OSC 8 hyperlink.
pub(crate) fn mark_underlined_hyperlink(buf: &mut Buffer, area: Rect, url: &str) {
    crate::terminal_hyperlinks::mark_underlined_hyperlink(buf, area, url);
}

use super::onboarding_screen::StepState;

mod headless_chatgpt_login;

#[derive(Clone)]
pub(crate) enum SignInState {
    PickMode,
    ChatGptContinueInBrowser(ContinueInBrowserState),
    #[allow(dead_code)]
    ChatGptDeviceCode(ContinueWithDeviceCodeState),
    ChatGptSuccessMessage,
    ChatGptSuccess,
    ApiKeyEntry(ApiKeyInputState),
    ApiKeyConfigured,
    ProviderSetup(ProviderSetupState),
    ProviderConfigured,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignInOption {
    ChatGpt,
    DeviceCode,
    ApiKey,
    CustomProvider,
}

const API_KEY_DISABLED_MESSAGE: &str = "API key login is disabled.";
fn onboarding_request_id() -> codex_app_server_protocol::RequestId {
    codex_app_server_protocol::RequestId::String(Uuid::new_v4().to_string())
}

pub(super) async fn cancel_login_attempt(
    request_handle: &AppServerRequestHandle,
    login_id: String,
) {
    let _ = request_handle
        .request_typed::<codex_app_server_protocol::CancelLoginAccountResponse>(
            ClientRequest::CancelLoginAccount {
                request_id: onboarding_request_id(),
                params: CancelLoginAccountParams { login_id },
            },
        )
        .await;
}

#[derive(Clone, Default)]
pub(crate) struct ApiKeyInputState {
    value: String,
    prepopulated_from_env: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderSetupField {
    Id,
    Name,
    BaseUrl,
    EnvKey,
    WireApi,
    FetchModels,
    Model,
    ContextWindow,
    Confirm,
}

#[derive(Clone)]
pub(crate) struct ProviderSetupState {
    field: ProviderSetupField,
    id: String,
    name: String,
    base_url: String,
    env_key: String,
    wire_api: WireApi,
    model: String,
    models: Vec<ProviderModelInfo>,
    selected_model_index: usize,
    context_window: i64,
    input: String,
    input_is_prefill: bool,
    is_saving: bool,
}

impl ProviderSetupState {
    fn new() -> Self {
        Self {
            field: ProviderSetupField::Id,
            id: String::new(),
            name: String::new(),
            base_url: String::new(),
            env_key: String::new(),
            wire_api: WireApi::Chat,
            model: "deepseek-chat".to_string(),
            models: Vec::new(),
            selected_model_index: 0,
            context_window: 262_144,
            input: String::new(),
            input_is_prefill: true,
            is_saving: false,
        }
    }

    fn start_field(&mut self, field: ProviderSetupField) {
        self.field = field;
        self.input = match field {
            ProviderSetupField::Id => self.id.clone(),
            ProviderSetupField::Name => self.name.clone(),
            ProviderSetupField::BaseUrl => self.base_url.clone(),
            ProviderSetupField::EnvKey => self.env_key.clone(),
            ProviderSetupField::WireApi => String::new(),
            ProviderSetupField::FetchModels => String::new(),
            ProviderSetupField::Model => String::new(),
            ProviderSetupField::ContextWindow => self.context_window.to_string(),
            ProviderSetupField::Confirm => String::new(),
        };
        self.input_is_prefill = field != ProviderSetupField::Confirm;
    }

    fn apply_input(&mut self) -> Result<(), String> {
        let value = self.input.trim();
        match self.field {
            ProviderSetupField::Id => {
                if value.is_empty() {
                    return Err("Provider id cannot be empty".to_string());
                }
                self.id = value.to_string();
                self.start_field(ProviderSetupField::Name);
            }
            ProviderSetupField::Name => {
                if value.is_empty() {
                    return Err("Provider name cannot be empty".to_string());
                }
                self.name = value.to_string();
                self.start_field(ProviderSetupField::BaseUrl);
            }
            ProviderSetupField::BaseUrl => {
                if value.is_empty() {
                    return Err("Base URL cannot be empty".to_string());
                }
                self.base_url = value.to_string();
                self.start_field(ProviderSetupField::EnvKey);
            }
            ProviderSetupField::EnvKey => {
                if value.is_empty() {
                    return Err("Env var cannot be empty; use - for no env var".to_string());
                }
                self.env_key = value.to_string();
                self.start_field(ProviderSetupField::WireApi);
            }
            ProviderSetupField::WireApi | ProviderSetupField::FetchModels => {}
            ProviderSetupField::Model => {
                if self.models.is_empty() {
                    return Err("No models were fetched for this provider".to_string());
                }
                if let Some(model) = self.models.get(self.selected_model_index) {
                    self.model = model.model_id.clone();
                    self.context_window = model
                        .context_window
                        .or(model.max_token_len)
                        .unwrap_or(self.context_window);
                }
                self.start_field(ProviderSetupField::ContextWindow);
            }
            ProviderSetupField::ContextWindow => {
                let parsed = value
                    .parse::<i64>()
                    .map_err(|_| "Context window must be a number".to_string())?;
                if parsed <= 0 {
                    return Err("Context window must be greater than zero".to_string());
                }
                self.context_window = parsed;
                self.start_field(ProviderSetupField::Confirm);
            }
            ProviderSetupField::Confirm => {}
        }
        Ok(())
    }

    fn set_fetched_models(&mut self, models: Vec<ProviderModelInfo>) -> Result<(), String> {
        if models.is_empty() {
            return Err("Provider returned no models.".to_string());
        }
        self.models = models;
        self.selected_model_index = 0;
        self.model = self.models[0].model_id.clone();
        self.context_window = self.models[0]
            .context_window
            .or(self.models[0].max_token_len)
            .unwrap_or(self.context_window);
        self.start_field(ProviderSetupField::Model);
        Ok(())
    }

    fn move_model_selection(&mut self, direction: ModelSelectionDirection) {
        if self.models.is_empty() {
            return;
        }
        self.selected_model_index = match direction {
            ModelSelectionDirection::Previous => self
                .selected_model_index
                .checked_sub(1)
                .unwrap_or(self.models.len() - 1),
            ModelSelectionDirection::Next => (self.selected_model_index + 1) % self.models.len(),
        };
        self.model = self.models[self.selected_model_index].model_id.clone();
    }

    fn move_form_selection(&mut self, direction: ModelSelectionDirection) {
        const ROWS: [ProviderSetupField; 6] = [
            ProviderSetupField::Id,
            ProviderSetupField::Name,
            ProviderSetupField::BaseUrl,
            ProviderSetupField::EnvKey,
            ProviderSetupField::WireApi,
            ProviderSetupField::FetchModels,
        ];
        let current = ROWS
            .iter()
            .position(|field| *field == self.field)
            .unwrap_or(0);
        let next = match direction {
            ModelSelectionDirection::Previous => current.checked_sub(1).unwrap_or(ROWS.len() - 1),
            ModelSelectionDirection::Next => (current + 1) % ROWS.len(),
        };
        self.start_field(ROWS[next]);
    }

    fn selected_form_field_mut(&mut self) -> Option<&mut String> {
        match self.field {
            ProviderSetupField::Id => Some(&mut self.id),
            ProviderSetupField::Name => Some(&mut self.name),
            ProviderSetupField::BaseUrl => Some(&mut self.base_url),
            ProviderSetupField::EnvKey => Some(&mut self.env_key),
            ProviderSetupField::WireApi
            | ProviderSetupField::FetchModels
            | ProviderSetupField::Model
            | ProviderSetupField::ContextWindow
            | ProviderSetupField::Confirm => None,
        }
    }

    fn toggle_wire_api(&mut self) {
        self.wire_api = match self.wire_api {
            WireApi::Chat => WireApi::Responses,
            WireApi::Responses => WireApi::Chat,
        };
    }

    fn provider(&self) -> ModelProviderInfo {
        let mut models = self.models.clone();
        if let Some(model) = models.iter_mut().find(|model| model.model_id == self.model) {
            model.context_window = Some(self.context_window);
        }

        ModelProviderInfo {
            name: self.name.trim().to_string(),
            base_url: Some(self.base_url.trim().to_string()),
            env_key: (self.env_key.trim() != "-").then(|| self.env_key.trim().to_string()),
            wire_api: self.wire_api,
            models,
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy)]
enum ModelSelectionDirection {
    Previous,
    Next,
}

#[derive(Clone)]
/// Used to manage the lifecycle of SpawnedLogin and ensure it gets cleaned up.
pub(crate) struct ContinueInBrowserState {
    login_id: String,
    auth_url: String,
}

#[derive(Clone)]
pub(crate) struct ContinueWithDeviceCodeState {
    request_id: String,
    login_id: Option<String>,
    verification_url: Option<String>,
    user_code: Option<String>,
}

impl ContinueWithDeviceCodeState {
    pub(crate) fn pending(request_id: String) -> Self {
        Self {
            request_id,
            login_id: None,
            verification_url: None,
            user_code: None,
        }
    }

    pub(crate) fn ready(
        request_id: String,
        login_id: String,
        verification_url: String,
        user_code: String,
    ) -> Self {
        Self {
            request_id,
            login_id: Some(login_id),
            verification_url: Some(verification_url),
            user_code: Some(user_code),
        }
    }

    pub(crate) fn login_id(&self) -> Option<&str> {
        self.login_id.as_deref()
    }

    pub(crate) fn is_showing_copyable_auth(&self) -> bool {
        self.verification_url
            .as_deref()
            .is_some_and(|url| !url.is_empty())
            && self
                .user_code
                .as_deref()
                .is_some_and(|user_code| !user_code.is_empty())
    }
}

impl KeyboardHandler for AuthModeWidget {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if self.handle_api_key_entry_key_event(&key_event) {
            return;
        }
        if self.handle_provider_setup_key_event(&key_event) {
            return;
        }

        if keys::MOVE_UP.is_pressed(key_event) {
            self.move_highlight(/*delta*/ -1);
            return;
        }
        if keys::MOVE_DOWN.is_pressed(key_event) {
            self.move_highlight(/*delta*/ 1);
            return;
        }
        if keys::SELECT_FIRST.is_pressed(key_event) {
            self.select_option_by_index(/*index*/ 0);
            return;
        }
        if keys::SELECT_SECOND.is_pressed(key_event) {
            self.select_option_by_index(/*index*/ 1);
            return;
        }
        if keys::SELECT_THIRD.is_pressed(key_event) {
            self.select_option_by_index(/*index*/ 2);
            return;
        }
        if keys::SELECT_FOURTH.is_pressed(key_event) {
            self.select_option_by_index(/*index*/ 3);
            return;
        }
        if keys::CONFIRM.is_pressed(key_event) {
            let sign_in_state = { (*self.sign_in_state.read().unwrap()).clone() };
            match sign_in_state {
                SignInState::PickMode => {
                    self.handle_sign_in_option(self.highlighted_mode);
                }
                SignInState::ChatGptSuccessMessage => {
                    *self.sign_in_state.write().unwrap() = SignInState::ChatGptSuccess;
                }
                _ => {}
            }
            return;
        }
        if keys::CANCEL.is_pressed(key_event) {
            tracing::info!("Cancel onboarding auth step");
            self.cancel_active_attempt();
        }
    }

    fn handle_paste(&mut self, pasted: String) {
        if self.handle_api_key_entry_paste(pasted.clone()) {
            return;
        }
        let _ = self.handle_provider_setup_paste(pasted);
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct AuthModeWidget {
    pub request_frame: FrameRequester,
    pub highlighted_mode: SignInOption,
    pub error: Arc<RwLock<Option<String>>>,
    pub sign_in_state: Arc<RwLock<SignInState>>,
    pub login_status: LoginStatus,
    pub app_server_request_handle: AppServerRequestHandle,
    pub forced_login_method: Option<ForcedLoginMethod>,
    pub animations_enabled: bool,
    pub animations_suppressed: Cell<bool>,
}

impl AuthModeWidget {
    pub(crate) fn set_animations_suppressed(&self, suppressed: bool) {
        self.animations_suppressed.set(suppressed);
    }

    pub(crate) fn should_suppress_animations(&self) -> bool {
        matches!(
            &*self.sign_in_state.read().unwrap(),
            SignInState::ChatGptContinueInBrowser(_) | SignInState::ChatGptDeviceCode(_)
        )
    }

    pub(crate) fn cancel_active_attempt(&self) {
        let mut sign_in_state = self.sign_in_state.write().unwrap();
        match &*sign_in_state {
            SignInState::ChatGptContinueInBrowser(state) => {
                let request_handle = self.app_server_request_handle.clone();
                let login_id = state.login_id.clone();
                tokio::spawn(async move {
                    cancel_login_attempt(&request_handle, login_id).await;
                });
            }
            SignInState::ChatGptDeviceCode(state) => {
                if let Some(login_id) = state.login_id().map(str::to_owned) {
                    let request_handle = self.app_server_request_handle.clone();
                    tokio::spawn(async move {
                        cancel_login_attempt(&request_handle, login_id).await;
                    });
                }
            }
            _ => return,
        }
        *sign_in_state = SignInState::PickMode;
        drop(sign_in_state);
        self.set_error(/*message*/ None);
        self.request_frame.schedule_frame();
    }

    fn set_error(&self, message: Option<String>) {
        *self.error.write().unwrap() = message;
    }

    fn error_message(&self) -> Option<String> {
        self.error.read().unwrap().clone()
    }

    /// Returns whether the auth flow is currently in API-key entry mode.
    pub(crate) fn is_api_key_entry_active(&self) -> bool {
        self.sign_in_state
            .read()
            .is_ok_and(|guard| matches!(&*guard, SignInState::ApiKeyEntry(_)))
    }

    /// Returns whether the API-key entry field currently contains any text.
    pub(crate) fn api_key_entry_has_text(&self) -> bool {
        self.sign_in_state.read().is_ok_and(
            |guard| matches!(&*guard, SignInState::ApiKeyEntry(state) if !state.value.is_empty()),
        )
    }

    /// Returns whether the custom-provider setup flow is editing a text field.
    pub(crate) fn is_provider_setup_text_entry_active(&self) -> bool {
        self.sign_in_state.read().is_ok_and(|guard| {
            matches!(
                &*guard,
                SignInState::ProviderSetup(state)
                    if !matches!(
                        state.field,
                        ProviderSetupField::Confirm | ProviderSetupField::WireApi | ProviderSetupField::Model
                    ) && !state.is_saving
            )
        })
    }

    /// Returns whether the custom-provider setup field currently contains text.
    pub(crate) fn provider_setup_has_text(&self) -> bool {
        self.sign_in_state.read().is_ok_and(|guard| {
            matches!(
                &*guard,
                SignInState::ProviderSetup(state) if !state.input.is_empty()
            )
        })
    }

    fn confirm_binding(&self) -> KeyBinding {
        keys::CONFIRM[0]
    }

    fn cancel_binding(&self) -> KeyBinding {
        keys::CANCEL[0]
    }

    fn is_api_login_allowed(&self) -> bool {
        !matches!(self.forced_login_method, Some(ForcedLoginMethod::Chatgpt))
    }

    fn is_chatgpt_login_allowed(&self) -> bool {
        !matches!(self.forced_login_method, Some(ForcedLoginMethod::Api))
    }

    fn displayed_sign_in_options(&self) -> Vec<SignInOption> {
        let mut options = vec![SignInOption::ChatGpt];
        if self.is_chatgpt_login_allowed() {
            options.push(SignInOption::DeviceCode);
        }
        if self.is_api_login_allowed() {
            options.push(SignInOption::ApiKey);
            options.push(SignInOption::CustomProvider);
        }
        options
    }

    fn selectable_sign_in_options(&self) -> Vec<SignInOption> {
        let mut options = Vec::new();
        if self.is_chatgpt_login_allowed() {
            options.push(SignInOption::ChatGpt);
            options.push(SignInOption::DeviceCode);
        }
        if self.is_api_login_allowed() {
            options.push(SignInOption::ApiKey);
            options.push(SignInOption::CustomProvider);
        }
        options
    }

    fn move_highlight(&mut self, delta: isize) {
        let options = self.selectable_sign_in_options();
        if options.is_empty() {
            return;
        }

        let current_index = options
            .iter()
            .position(|option| *option == self.highlighted_mode)
            .unwrap_or(0);
        let next_index =
            (current_index as isize + delta).rem_euclid(options.len() as isize) as usize;
        self.highlighted_mode = options[next_index];
    }

    fn select_option_by_index(&mut self, index: usize) {
        let options = self.displayed_sign_in_options();
        if let Some(option) = options.get(index).copied() {
            self.handle_sign_in_option(option);
        }
    }

    fn handle_sign_in_option(&mut self, option: SignInOption) {
        match option {
            SignInOption::ChatGpt => {
                if self.is_chatgpt_login_allowed() {
                    self.start_chatgpt_login();
                }
            }
            SignInOption::DeviceCode => {
                if self.is_chatgpt_login_allowed() {
                    self.start_device_code_login();
                }
            }
            SignInOption::ApiKey => {
                if self.is_api_login_allowed() {
                    self.start_api_key_entry();
                } else {
                    self.disallow_api_login();
                }
            }
            SignInOption::CustomProvider => {
                if self.is_api_login_allowed() {
                    self.start_provider_setup();
                } else {
                    self.disallow_api_login();
                }
            }
        }
    }

    fn disallow_api_login(&mut self) {
        self.highlighted_mode = SignInOption::ChatGpt;
        self.set_error(Some(API_KEY_DISABLED_MESSAGE.to_string()));
        *self.sign_in_state.write().unwrap() = SignInState::PickMode;
        self.request_frame.schedule_frame();
    }

    fn render_pick_mode(&self, area: Rect, buf: &mut Buffer) {
        let mut lines: Vec<Line> = vec![
            Line::from(vec![
                "  ".into(),
                "Sign in with ChatGPT to use Codex as part of your paid plan".into(),
            ]),
            Line::from(vec![
                "  ".into(),
                "or connect an API key for usage-based billing".into(),
            ]),
            "".into(),
        ];

        let create_mode_item = |idx: usize,
                                selected_mode: SignInOption,
                                text: &str,
                                description: &str|
         -> Vec<Line<'static>> {
            let is_selected = self.highlighted_mode == selected_mode;
            let caret = if is_selected { ">" } else { " " };

            let line1 = if is_selected {
                Line::from(vec![
                    format!("{caret} {index}. ", index = idx + 1).cyan().dim(),
                    text.to_string().cyan(),
                ])
            } else {
                format!("  {index}. {text}", index = idx + 1).into()
            };

            let line2 = if is_selected {
                Line::from(format!("     {description}"))
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::DIM)
            } else {
                Line::from(format!("     {description}"))
                    .style(Style::default().add_modifier(Modifier::DIM))
            };

            vec![line1, line2]
        };

        let chatgpt_description = if !self.is_chatgpt_login_allowed() {
            "ChatGPT login is disabled"
        } else {
            "Usage included with Plus, Pro, Business, and Enterprise plans"
        };
        let device_code_description = "Sign in from another device with a one-time code";

        for (idx, option) in self.displayed_sign_in_options().into_iter().enumerate() {
            match option {
                SignInOption::ChatGpt => {
                    lines.extend(create_mode_item(
                        idx,
                        option,
                        "Sign in with ChatGPT",
                        chatgpt_description,
                    ));
                }
                SignInOption::DeviceCode => {
                    lines.extend(create_mode_item(
                        idx,
                        option,
                        "Sign in with Device Code",
                        device_code_description,
                    ));
                }
                SignInOption::ApiKey => {
                    lines.extend(create_mode_item(
                        idx,
                        option,
                        "Provide your own API key",
                        "Pay for what you use",
                    ));
                }
                SignInOption::CustomProvider => {
                    lines.extend(create_mode_item(
                        idx,
                        option,
                        "Configure custom provider",
                        "Use an OpenAI-compatible provider such as DeepSeek or OpenRouter",
                    ));
                }
            }
            lines.push("".into());
        }

        if !self.is_api_login_allowed() {
            lines.push(
                "  API key login is disabled by this workspace. Sign in with ChatGPT to continue."
                    .dim()
                    .into(),
            );
            lines.push("".into());
        }
        lines.push(Line::from(vec![
            "  Press ".dim(),
            self.confirm_binding().into(),
            " to continue".dim(),
        ]));
        if let Some(err) = self.error_message() {
            lines.push("".into());
            lines.push(err.red().into());
        }

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn render_continue_in_browser(&self, area: Rect, buf: &mut Buffer) {
        let mut spans = vec!["  ".into()];
        if self.animations_enabled && !self.animations_suppressed.get() {
            // Schedule a follow-up frame to keep the shimmer animation going.
            self.request_frame
                .schedule_frame_in(std::time::Duration::from_millis(100));
            spans.extend(shimmer_text(
                "Finish signing in via your browser",
                MotionMode::Animated,
            ));
        } else {
            spans.push("Finish signing in via your browser".into());
        }
        let mut lines = vec![spans.into(), "".into()];

        let sign_in_state = self.sign_in_state.read().unwrap();
        let auth_url = if let SignInState::ChatGptContinueInBrowser(state) = &*sign_in_state
            && !state.auth_url.is_empty()
        {
            lines.push("  If the link doesn't open automatically, open the following link to authenticate:".into());
            lines.push("".into());
            lines.push(Line::from(vec![
                "  ".into(),
                state.auth_url.as_str().cyan().underlined(),
            ]));
            lines.push("".into());
            lines.push(Line::from(vec![
                "  On a remote or headless machine? Press ".into(),
                self.cancel_binding().into(),
                " and choose ".into(),
                "Sign in with Device Code".cyan(),
                ".".into(),
            ]));
            lines.push("".into());
            Some(state.auth_url.clone())
        } else {
            None
        };

        lines.push(Line::from(vec![
            "  Press ".dim(),
            self.cancel_binding().into(),
            " to cancel".dim(),
        ]));
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);

        // Wrap cyan+underlined URL cells with OSC 8 so the terminal treats
        // the entire region as a single clickable hyperlink.
        if let Some(url) = &auth_url {
            mark_url_hyperlink(buf, area, url);
        }
    }

    fn render_chatgpt_success_message(&self, area: Rect, buf: &mut Buffer) {
        let lines = vec![
            "✓ Signed in with your ChatGPT account"
                .fg(Color::Green)
                .into(),
            "".into(),
            "  Before you start:".into(),
            "".into(),
            "  Decide how much autonomy you want to grant Codex".into(),
            Line::from(vec![
                "  For more details see the ".into(),
                crate::terminal_hyperlinks::osc8_hyperlink(
                    "https://developers.openai.com/codex/security",
                    "Codex docs",
                )
                .underlined(),
            ])
            .dim(),
            "".into(),
            "  Codex can make mistakes".into(),
            "  Review the code it writes and commands it runs"
                .dim()
                .into(),
            "".into(),
            "  Powered by your ChatGPT account".into(),
            Line::from(vec![
                "  Uses your plan's rate limits and ".into(),
                crate::terminal_hyperlinks::osc8_hyperlink(
                    "https://chatgpt.com/#settings",
                    "training data preferences",
                )
                .underlined(),
            ])
            .dim(),
            "".into(),
            Line::from(vec![
                "  Press ".fg(Color::Cyan),
                self.confirm_binding().into(),
                " to continue".fg(Color::Cyan),
            ]),
        ];

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn render_chatgpt_success(&self, area: Rect, buf: &mut Buffer) {
        let lines = vec![
            "✓ Signed in with your ChatGPT account"
                .fg(Color::Green)
                .into(),
        ];

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn render_api_key_configured(&self, area: Rect, buf: &mut Buffer) {
        let lines = vec![
            "✓ API key configured".fg(Color::Green).into(),
            "".into(),
            "  Codex will use usage-based billing with your API key.".into(),
        ];

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn render_api_key_entry(&self, area: Rect, buf: &mut Buffer, state: &ApiKeyInputState) {
        let [intro_area, input_area, footer_area] = Layout::vertical([
            Constraint::Min(4),
            Constraint::Length(3),
            Constraint::Min(2),
        ])
        .areas(area);

        let mut intro_lines: Vec<Line> = vec![
            Line::from(vec![
                "> ".into(),
                "Use your own OpenAI API key for usage-based billing".bold(),
            ]),
            "".into(),
            "  Paste or type your API key below. It will be stored locally in auth.json.".into(),
            "".into(),
        ];
        if state.prepopulated_from_env {
            intro_lines.push("  Detected OPENAI_API_KEY environment variable.".into());
            intro_lines.push(
                "  Paste a different key if you prefer to use another account."
                    .dim()
                    .into(),
            );
            intro_lines.push("".into());
        }
        Paragraph::new(intro_lines)
            .wrap(Wrap { trim: false })
            .render(intro_area, buf);

        let content_line: Line = if state.value.is_empty() {
            vec!["Paste or type your API key".dim()].into()
        } else {
            Line::from(state.value.clone())
        };
        Paragraph::new(content_line)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title("API key")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .render(input_area, buf);

        let mut footer_lines: Vec<Line> = vec![
            Line::from(vec![
                "  Press ".dim(),
                self.confirm_binding().into(),
                " to save".dim(),
            ]),
            Line::from(vec![
                "  Press ".dim(),
                self.cancel_binding().into(),
                " to go back".dim(),
            ]),
        ];
        if let Some(error) = self.error_message() {
            footer_lines.push("".into());
            footer_lines.push(error.red().into());
        }
        Paragraph::new(footer_lines)
            .wrap(Wrap { trim: false })
            .render(footer_area, buf);
    }

    fn render_provider_setup(&self, area: Rect, buf: &mut Buffer, state: &ProviderSetupState) {
        let [intro_area, input_area, footer_area] = Layout::vertical([
            Constraint::Min(8),
            Constraint::Length(match state.field {
                ProviderSetupField::Confirm => 0,
                ProviderSetupField::Id
                | ProviderSetupField::Name
                | ProviderSetupField::BaseUrl
                | ProviderSetupField::EnvKey
                | ProviderSetupField::WireApi
                | ProviderSetupField::FetchModels => 10,
                ProviderSetupField::Model => 7,
                _ => 3,
            }),
            Constraint::Min(4),
        ])
        .areas(area);

        let mut intro_lines: Vec<Line> = vec![
            Line::from(vec![
                "> ".into(),
                "Configure a custom OpenAI-compatible provider".bold(),
            ]),
            "".into(),
            "  Provider settings are stored in config.toml.".into(),
            "  Provider API keys are read from the env var you configure below."
                .dim()
                .into(),
            "".into(),
            Line::from(vec!["  id: ".dim(), state.id.clone().into()]),
            Line::from(vec!["  name: ".dim(), state.name.clone().into()]),
            Line::from(vec!["  base_url: ".dim(), state.base_url.clone().into()]),
            Line::from(vec!["  env_key: ".dim(), state.env_key.clone().into()]),
            Line::from(vec![
                "  wire_api: ".dim(),
                state.wire_api.to_string().into(),
            ]),
            Line::from(vec!["  model: ".dim(), state.model.clone().into()]),
            Line::from(vec![
                "  context_window: ".dim(),
                state.context_window.to_string().into(),
            ]),
            "".into(),
        ];
        if state.field == ProviderSetupField::Confirm {
            intro_lines.push(if state.is_saving {
                "  Saving provider configuration...".cyan().into()
            } else {
                Line::from(vec![
                    "  Press ".cyan(),
                    self.confirm_binding().into(),
                    " to confirm".cyan(),
                ])
            });
        }
        Paragraph::new(intro_lines)
            .wrap(Wrap { trim: false })
            .render(intro_area, buf);

        match state.field {
            ProviderSetupField::Confirm => {}
            ProviderSetupField::Id
            | ProviderSetupField::Name
            | ProviderSetupField::BaseUrl
            | ProviderSetupField::EnvKey
            | ProviderSetupField::WireApi
            | ProviderSetupField::FetchModels => {
                Paragraph::new(provider_setup_form_lines(state))
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .title("Provider")
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .border_style(Style::default().fg(Color::Cyan)),
                    )
                    .render(input_area, buf);
            }
            ProviderSetupField::Model => {
                Paragraph::new(provider_setup_model_lines(state))
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .title(provider_setup_field_title(state.field))
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .border_style(Style::default().fg(Color::Cyan)),
                    )
                    .render(input_area, buf);
            }
            _ => {
                let content_line: Line = if state.input.is_empty() {
                    provider_setup_placeholder(state.field).dim().into()
                } else {
                    Line::from(state.input.clone())
                };
                Paragraph::new(content_line)
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .title(provider_setup_field_title(state.field))
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .border_style(Style::default().fg(Color::Cyan)),
                    )
                    .render(input_area, buf);
            }
        }

        let mut footer_lines: Vec<Line> = Vec::new();
        if matches!(
            state.field,
            ProviderSetupField::Id
                | ProviderSetupField::Name
                | ProviderSetupField::BaseUrl
                | ProviderSetupField::EnvKey
                | ProviderSetupField::WireApi
                | ProviderSetupField::FetchModels
        ) {
            footer_lines.push(
                "  Use ↑/↓ to choose a field; ←/→ changes wire type"
                    .dim()
                    .into(),
            );
        } else if state.field == ProviderSetupField::Model {
            footer_lines.push("  Use ↑/↓ or 1/2 to choose a model".dim().into());
        }
        footer_lines.extend([
            Line::from(vec![
                "  Press ".dim(),
                self.confirm_binding().into(),
                if state.field == ProviderSetupField::Confirm {
                    " to save".dim()
                } else if state.field == ProviderSetupField::FetchModels {
                    " to fetch models".dim()
                } else {
                    " to continue".dim()
                },
            ]),
            Line::from(vec![
                "  Press ".dim(),
                self.cancel_binding().into(),
                " to cancel".dim(),
            ]),
        ]);
        if let Some(error) = self.error_message() {
            footer_lines.push("".into());
            footer_lines.push(error.red().into());
        }
        Paragraph::new(footer_lines)
            .wrap(Wrap { trim: false })
            .render(footer_area, buf);
    }

    fn render_provider_configured(&self, area: Rect, buf: &mut Buffer) {
        let lines = vec![
            "✓ Custom provider configured".fg(Color::Green).into(),
            "".into(),
            "  Codex will use the selected provider and model from config.toml.".into(),
        ];

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn handle_api_key_entry_key_event(&mut self, key_event: &KeyEvent) -> bool {
        let mut should_save: Option<String> = None;
        let mut should_request_frame = false;

        {
            let mut guard = self.sign_in_state.write().unwrap();
            if let SignInState::ApiKeyEntry(state) = &mut *guard {
                if keys::CANCEL.is_pressed(*key_event) {
                    *guard = SignInState::PickMode;
                    self.set_error(/*message*/ None);
                    should_request_frame = true;
                } else if keys::CONFIRM.is_pressed(*key_event) {
                    let trimmed = state.value.trim().to_string();
                    if trimmed.is_empty() {
                        self.set_error(Some("API key cannot be empty".to_string()));
                        should_request_frame = true;
                    } else {
                        should_save = Some(trimmed);
                    }
                } else {
                    match key_event.code {
                        KeyCode::Backspace => {
                            if state.prepopulated_from_env {
                                state.value.clear();
                                state.prepopulated_from_env = false;
                            } else {
                                state.value.pop();
                            }
                            self.set_error(/*message*/ None);
                            should_request_frame = true;
                        }
                        KeyCode::Char(c)
                            if key_event.kind == KeyEventKind::Press
                                && !key_event.modifiers.contains(KeyModifiers::SUPER)
                                && !key_event.modifiers.contains(KeyModifiers::CONTROL)
                                && !key_event.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            if state.prepopulated_from_env {
                                state.value.clear();
                                state.prepopulated_from_env = false;
                            }
                            state.value.push(c);
                            self.set_error(/*message*/ None);
                            should_request_frame = true;
                        }
                        _ => {}
                    }
                }
                // handled; let guard drop before potential save
            } else {
                return false;
            }
        }

        if let Some(api_key) = should_save {
            self.save_api_key(api_key);
        } else if should_request_frame {
            self.request_frame.schedule_frame();
        }
        true
    }

    fn handle_api_key_entry_paste(&mut self, pasted: String) -> bool {
        let trimmed = pasted.trim();
        if trimmed.is_empty() {
            return false;
        }

        let mut guard = self.sign_in_state.write().unwrap();
        if let SignInState::ApiKeyEntry(state) = &mut *guard {
            if state.prepopulated_from_env {
                state.value = trimmed.to_string();
                state.prepopulated_from_env = false;
            } else {
                state.value.push_str(trimmed);
            }
            self.set_error(/*message*/ None);
        } else {
            return false;
        }

        drop(guard);
        self.request_frame.schedule_frame();
        true
    }

    fn start_api_key_entry(&mut self) {
        if !self.is_api_login_allowed() {
            self.disallow_api_login();
            return;
        }
        self.set_error(/*message*/ None);
        let prefill_from_env = read_openai_api_key_from_env();
        let mut guard = self.sign_in_state.write().unwrap();
        match &mut *guard {
            SignInState::ApiKeyEntry(state) => {
                if state.value.is_empty() {
                    if let Some(prefill) = prefill_from_env {
                        state.value = prefill;
                        state.prepopulated_from_env = true;
                    } else {
                        state.prepopulated_from_env = false;
                    }
                }
            }
            _ => {
                *guard = SignInState::ApiKeyEntry(ApiKeyInputState {
                    value: prefill_from_env.clone().unwrap_or_default(),
                    prepopulated_from_env: prefill_from_env.is_some(),
                });
            }
        }
        drop(guard);
        self.request_frame.schedule_frame();
    }

    fn save_api_key(&mut self, api_key: String) {
        if !self.is_api_login_allowed() {
            self.disallow_api_login();
            return;
        }
        self.set_error(/*message*/ None);
        let request_handle = self.app_server_request_handle.clone();
        let sign_in_state = self.sign_in_state.clone();
        let error = self.error.clone();
        let request_frame = self.request_frame.clone();
        tokio::spawn(async move {
            match request_handle
                .request_typed::<LoginAccountResponse>(ClientRequest::LoginAccount {
                    request_id: onboarding_request_id(),
                    params: LoginAccountParams::ApiKey {
                        api_key: api_key.clone(),
                    },
                })
                .await
            {
                Ok(LoginAccountResponse::ApiKey {}) => {
                    *error.write().unwrap() = None;
                    *sign_in_state.write().unwrap() = SignInState::ApiKeyConfigured;
                }
                Ok(other) => {
                    *error.write().unwrap() = Some(format!(
                        "Unexpected account/login/start response: {other:?}"
                    ));
                    *sign_in_state.write().unwrap() = SignInState::ApiKeyEntry(ApiKeyInputState {
                        value: api_key,
                        prepopulated_from_env: false,
                    });
                }
                Err(err) => {
                    *error.write().unwrap() = Some(format!("Failed to save API key: {err}"));
                    *sign_in_state.write().unwrap() = SignInState::ApiKeyEntry(ApiKeyInputState {
                        value: api_key,
                        prepopulated_from_env: false,
                    });
                }
            }
            request_frame.schedule_frame();
        });
        self.request_frame.schedule_frame();
    }

    fn start_provider_setup(&mut self) {
        if !self.is_api_login_allowed() {
            self.disallow_api_login();
            return;
        }
        self.set_error(/*message*/ None);
        *self.sign_in_state.write().unwrap() =
            SignInState::ProviderSetup(ProviderSetupState::new());
        self.request_frame.schedule_frame();
    }

    fn handle_provider_setup_key_event(&mut self, key_event: &KeyEvent) -> bool {
        let mut should_save: Option<ProviderSetupState> = None;
        let mut should_fetch: Option<ProviderSetupState> = None;
        let mut should_request_frame = false;
        let mut error_message: Option<String> = None;

        {
            let mut guard = self.sign_in_state.write().unwrap();
            let SignInState::ProviderSetup(state) = &mut *guard else {
                return false;
            };

            if state.is_saving {
                return true;
            }

            if matches!(
                state.field,
                ProviderSetupField::Id
                    | ProviderSetupField::Name
                    | ProviderSetupField::BaseUrl
                    | ProviderSetupField::EnvKey
                    | ProviderSetupField::WireApi
                    | ProviderSetupField::FetchModels
            ) {
                if keys::MOVE_UP.is_pressed(*key_event) {
                    state.move_form_selection(ModelSelectionDirection::Previous);
                    self.set_error(/*message*/ None);
                    should_request_frame = true;
                } else if keys::MOVE_DOWN.is_pressed(*key_event) {
                    state.move_form_selection(ModelSelectionDirection::Next);
                    self.set_error(/*message*/ None);
                    should_request_frame = true;
                } else if matches!(key_event.code, KeyCode::Left | KeyCode::Right)
                    && state.field == ProviderSetupField::WireApi
                {
                    state.toggle_wire_api();
                    self.set_error(/*message*/ None);
                    should_request_frame = true;
                } else if keys::CONFIRM.is_pressed(*key_event)
                    && state.field == ProviderSetupField::FetchModels
                {
                    let provider = state.provider();
                    if built_in_model_providers(None).contains_key(&state.id) {
                        error_message = Some(format!(
                            "Provider id '{}' is built in. Choose a custom id such as '{}-custom'.",
                            state.id, state.id
                        ));
                    } else if let Err(err) = provider.validate() {
                        error_message = Some(format!("Invalid provider: {err}"));
                    } else {
                        state.is_saving = true;
                        should_fetch = Some(state.clone());
                        self.set_error(/*message*/ None);
                    }
                    should_request_frame = true;
                } else if keys::CANCEL.is_pressed(*key_event) {
                    *guard = SignInState::PickMode;
                    self.set_error(/*message*/ None);
                    self.highlighted_mode = SignInOption::CustomProvider;
                    should_request_frame = true;
                } else {
                    match key_event.code {
                        KeyCode::Backspace => {
                            if let Some(value) = state.selected_form_field_mut() {
                                value.pop();
                                self.set_error(/*message*/ None);
                                should_request_frame = true;
                            }
                        }
                        KeyCode::Char(c)
                            if key_event.kind == KeyEventKind::Press
                                && !key_event.modifiers.contains(KeyModifiers::SUPER)
                                && !key_event.modifiers.contains(KeyModifiers::CONTROL)
                                && !key_event.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            if let Some(value) = state.selected_form_field_mut() {
                                value.push(c);
                                self.set_error(/*message*/ None);
                                should_request_frame = true;
                            }
                        }
                        _ => {}
                    }
                }
            } else if state.field == ProviderSetupField::Model {
                if keys::MOVE_UP.is_pressed(*key_event) || keys::SELECT_FIRST.is_pressed(*key_event)
                {
                    state.move_model_selection(ModelSelectionDirection::Previous);
                    self.set_error(/*message*/ None);
                    should_request_frame = true;
                } else if keys::MOVE_DOWN.is_pressed(*key_event)
                    || keys::SELECT_SECOND.is_pressed(*key_event)
                {
                    state.move_model_selection(ModelSelectionDirection::Next);
                    self.set_error(/*message*/ None);
                    should_request_frame = true;
                } else if keys::CONFIRM.is_pressed(*key_event) {
                    match state.apply_input() {
                        Ok(()) => self.set_error(/*message*/ None),
                        Err(err) => error_message = Some(err),
                    }
                    should_request_frame = true;
                } else if keys::CANCEL.is_pressed(*key_event) {
                    state.start_field(ProviderSetupField::WireApi);
                    self.set_error(/*message*/ None);
                    should_request_frame = true;
                }
            } else if keys::CANCEL.is_pressed(*key_event) {
                *guard = SignInState::PickMode;
                self.set_error(/*message*/ None);
                self.highlighted_mode = SignInOption::CustomProvider;
                should_request_frame = true;
            } else if keys::CONFIRM.is_pressed(*key_event) {
                if state.field == ProviderSetupField::Confirm {
                    if built_in_model_providers(None).contains_key(&state.id) {
                        error_message = Some(format!(
                            "Provider id '{}' is built in. Choose a custom id such as '{}-custom'.",
                            state.id, state.id
                        ));
                    } else {
                        let provider = state.provider();
                        if let Err(err) = provider.validate() {
                            error_message = Some(format!("Invalid provider: {err}"));
                        } else {
                            state.is_saving = true;
                            should_save = Some(state.clone());
                        }
                    }
                    should_request_frame = true;
                } else {
                    match state.apply_input() {
                        Ok(()) => self.set_error(/*message*/ None),
                        Err(err) => error_message = Some(err),
                    }
                    should_request_frame = true;
                }
            } else {
                match key_event.code {
                    KeyCode::Backspace => {
                        if state.input_is_prefill {
                            state.input.clear();
                            state.input_is_prefill = false;
                        } else {
                            state.input.pop();
                        }
                        self.set_error(/*message*/ None);
                        should_request_frame = true;
                    }
                    KeyCode::Char(c)
                        if key_event.kind == KeyEventKind::Press
                            && !key_event.modifiers.contains(KeyModifiers::SUPER)
                            && !key_event.modifiers.contains(KeyModifiers::CONTROL)
                            && !key_event.modifiers.contains(KeyModifiers::ALT)
                            && !matches!(
                                state.field,
                                ProviderSetupField::Confirm
                                    | ProviderSetupField::WireApi
                                    | ProviderSetupField::Model
                            ) =>
                    {
                        if state.input_is_prefill {
                            state.input.clear();
                            state.input_is_prefill = false;
                        }
                        state.input.push(c);
                        self.set_error(/*message*/ None);
                        should_request_frame = true;
                    }
                    _ => {}
                }
            }
        }

        if let Some(err) = error_message {
            self.set_error(Some(err));
        }
        if let Some(state) = should_fetch {
            self.fetch_provider_models(state);
        } else if let Some(state) = should_save {
            self.save_provider_setup(state);
        } else if should_request_frame {
            self.request_frame.schedule_frame();
        }
        true
    }

    fn handle_provider_setup_paste(&mut self, pasted: String) -> bool {
        let pasted = pasted.trim();
        if pasted.is_empty() {
            return false;
        }

        let mut guard = self.sign_in_state.write().unwrap();
        let SignInState::ProviderSetup(state) = &mut *guard else {
            return false;
        };
        if matches!(
            state.field,
            ProviderSetupField::Confirm | ProviderSetupField::WireApi | ProviderSetupField::Model
        ) || state.is_saving
        {
            return true;
        }
        if state.input_is_prefill {
            state.input.clear();
            state.input_is_prefill = false;
        }
        state.input.push_str(pasted);
        drop(guard);
        self.set_error(/*message*/ None);
        self.request_frame.schedule_frame();
        true
    }

    fn fetch_provider_models(&mut self, state: ProviderSetupState) {
        let sign_in_state = self.sign_in_state.clone();
        let error = self.error.clone();
        let request_frame = self.request_frame.clone();
        tokio::spawn(async move {
            let provider = state.provider();
            let fetch_result = async {
                let models = fetch_remote_provider_models(provider, /*auth_manager*/ None)
                    .await
                    .map_err(|err| err.to_string())?;
                if models.is_empty() {
                    return Err("Provider returned no models.".to_string());
                }
                Ok(models
                    .into_iter()
                    .map(codex_protocol::openai_models::ModelPreset::from)
                    .map(|model| ProviderModelInfo::from(&model))
                    .collect::<Vec<_>>())
            }
            .await;

            let mut restored = state;
            restored.is_saving = false;
            match fetch_result.and_then(|models| {
                restored.set_fetched_models(models)?;
                Ok(())
            }) {
                Ok(()) => {
                    *error.write().unwrap() = None;
                    *sign_in_state.write().unwrap() = SignInState::ProviderSetup(restored);
                }
                Err(err) => {
                    *error.write().unwrap() = Some(format!(
                        "Failed to fetch provider models: {err}. Check the base URL, API key env var, and wire API."
                    ));
                    restored.start_field(ProviderSetupField::WireApi);
                    *sign_in_state.write().unwrap() = SignInState::ProviderSetup(restored);
                }
            }
            request_frame.schedule_frame();
        });
        self.request_frame.schedule_frame();
    }

    fn save_provider_setup(&mut self, state: ProviderSetupState) {
        let request_handle = self.app_server_request_handle.clone();
        let sign_in_state = self.sign_in_state.clone();
        let error = self.error.clone();
        let request_frame = self.request_frame.clone();
        tokio::spawn(async move {
            let provider = state.provider();
            let mut edits = match config_update::build_model_provider_edit(&state.id, &provider) {
                Ok(edit) => vec![edit],
                Err(err) => {
                    *error.write().unwrap() = Some(format!(
                        "Failed to serialize provider '{}': {err}",
                        state.id
                    ));
                    let mut restored = state;
                    restored.is_saving = false;
                    *sign_in_state.write().unwrap() = SignInState::ProviderSetup(restored);
                    request_frame.schedule_frame();
                    return;
                }
            };
            edits.push(config_update::build_model_provider_selection_edit(
                &state.id,
            ));
            edits.push(config_update::build_model_selection_edit(&state.model));

            match config_update::write_config_batch(request_handle, edits).await {
                Ok(_) => {
                    *error.write().unwrap() = None;
                    *sign_in_state.write().unwrap() = SignInState::ProviderConfigured;
                }
                Err(err) => {
                    let error_message = config_update::format_config_error(&err);
                    *error.write().unwrap() = Some(format!(
                        "Failed to save provider configuration: {error_message}"
                    ));
                    let mut restored = state;
                    restored.is_saving = false;
                    *sign_in_state.write().unwrap() = SignInState::ProviderSetup(restored);
                }
            }
            request_frame.schedule_frame();
        });
        self.request_frame.schedule_frame();
    }

    fn handle_existing_chatgpt_login(&mut self) -> bool {
        if matches!(
            self.login_status,
            LoginStatus::AuthMode(auth_mode) if auth_mode.has_chatgpt_account()
        ) {
            *self.sign_in_state.write().unwrap() = SignInState::ChatGptSuccess;
            self.request_frame.schedule_frame();
            true
        } else {
            false
        }
    }

    /// Kicks off the ChatGPT auth flow and keeps the UI state consistent with the attempt.
    fn start_chatgpt_login(&mut self) {
        // If we're already authenticated with ChatGPT, don't start a new login –
        // just proceed to the success message flow.
        if self.handle_existing_chatgpt_login() {
            return;
        }

        self.set_error(/*message*/ None);
        let request_handle = self.app_server_request_handle.clone();
        let sign_in_state = self.sign_in_state.clone();
        let error = self.error.clone();
        let request_frame = self.request_frame.clone();
        tokio::spawn(async move {
            match request_handle
                .request_typed::<LoginAccountResponse>(ClientRequest::LoginAccount {
                    request_id: onboarding_request_id(),
                    params: LoginAccountParams::Chatgpt {
                        codex_streamlined_login: false,
                    },
                })
                .await
            {
                Ok(LoginAccountResponse::Chatgpt { login_id, auth_url }) => {
                    maybe_open_auth_url_in_browser(&request_handle, &auth_url);
                    *error.write().unwrap() = None;
                    *sign_in_state.write().unwrap() =
                        SignInState::ChatGptContinueInBrowser(ContinueInBrowserState {
                            login_id,
                            auth_url,
                        });
                }
                Ok(other) => {
                    *sign_in_state.write().unwrap() = SignInState::PickMode;
                    *error.write().unwrap() = Some(format!(
                        "Unexpected account/login/start response: {other:?}"
                    ));
                }
                Err(err) => {
                    *sign_in_state.write().unwrap() = SignInState::PickMode;
                    *error.write().unwrap() = Some(err.to_string());
                }
            }
            request_frame.schedule_frame();
        });
    }

    fn start_device_code_login(&mut self) {
        if self.handle_existing_chatgpt_login() {
            return;
        }

        self.set_error(/*message*/ None);
        headless_chatgpt_login::start_headless_chatgpt_login(self);
    }

    pub(crate) fn on_account_login_completed(
        &mut self,
        notification: AccountLoginCompletedNotification,
    ) {
        let Some(login_id) = notification.login_id else {
            return;
        };
        let guard = self.sign_in_state.read().unwrap();
        let is_matching_login = matches!(
            &*guard,
            SignInState::ChatGptContinueInBrowser(state) if state.login_id == login_id
        ) || matches!(
            &*guard,
            SignInState::ChatGptDeviceCode(state) if state.login_id() == Some(login_id.as_str())
        );
        drop(guard);
        if !is_matching_login {
            return;
        }

        if notification.success {
            self.set_error(/*message*/ None);
            *self.sign_in_state.write().unwrap() = SignInState::ChatGptSuccessMessage;
        } else {
            self.set_error(notification.error);
            *self.sign_in_state.write().unwrap() = SignInState::PickMode;
        }
        self.request_frame.schedule_frame();
    }

    pub(crate) fn on_account_updated(&mut self, notification: AccountUpdatedNotification) {
        self.login_status = notification
            .auth_mode
            .map(|auth_mode| {
                LoginStatus::AuthMode(match auth_mode {
                    ApiAuthMode::ApiKey => AuthMode::ApiKey,
                    ApiAuthMode::Chatgpt => AuthMode::Chatgpt,
                    ApiAuthMode::ChatgptAuthTokens => AuthMode::ChatgptAuthTokens,
                    ApiAuthMode::AgentIdentity => AuthMode::AgentIdentity,
                    ApiAuthMode::PersonalAccessToken => AuthMode::PersonalAccessToken,
                    ApiAuthMode::BedrockApiKey => AuthMode::BedrockApiKey,
                })
            })
            .unwrap_or(LoginStatus::NotAuthenticated);
    }
}

impl StepStateProvider for AuthModeWidget {
    fn get_step_state(&self) -> StepState {
        let sign_in_state = self.sign_in_state.read().unwrap();
        match &*sign_in_state {
            SignInState::PickMode
            | SignInState::ApiKeyEntry(_)
            | SignInState::ProviderSetup(_)
            | SignInState::ChatGptContinueInBrowser(_)
            | SignInState::ChatGptDeviceCode(_)
            | SignInState::ChatGptSuccessMessage => StepState::InProgress,
            SignInState::ChatGptSuccess
            | SignInState::ApiKeyConfigured
            | SignInState::ProviderConfigured => StepState::Complete,
        }
    }
}

impl WidgetRef for AuthModeWidget {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let sign_in_state = self.sign_in_state.read().unwrap();
        match &*sign_in_state {
            SignInState::PickMode => {
                self.render_pick_mode(area, buf);
            }
            SignInState::ChatGptContinueInBrowser(_) => {
                self.render_continue_in_browser(area, buf);
            }
            SignInState::ChatGptDeviceCode(state) => {
                headless_chatgpt_login::render_device_code_login(self, area, buf, state);
            }
            SignInState::ChatGptSuccessMessage => {
                self.render_chatgpt_success_message(area, buf);
            }
            SignInState::ChatGptSuccess => {
                self.render_chatgpt_success(area, buf);
            }
            SignInState::ApiKeyEntry(state) => {
                self.render_api_key_entry(area, buf, state);
            }
            SignInState::ApiKeyConfigured => {
                self.render_api_key_configured(area, buf);
            }
            SignInState::ProviderSetup(state) => {
                self.render_provider_setup(area, buf, state);
            }
            SignInState::ProviderConfigured => {
                self.render_provider_configured(area, buf);
            }
        }
    }
}

fn provider_setup_form_lines(state: &ProviderSetupState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(provider_setup_form_row(
        state.field == ProviderSetupField::Id,
        "id",
        &state.id,
        "provider-id",
    ));
    lines.push(provider_setup_form_row(
        state.field == ProviderSetupField::Name,
        "name",
        &state.name,
        "My Provider",
    ));
    lines.push(provider_setup_form_row(
        state.field == ProviderSetupField::BaseUrl,
        "url",
        &state.base_url,
        "https://api.example.com/v1",
    ));
    lines.push(provider_setup_form_row(
        state.field == ProviderSetupField::EnvKey,
        "apikey env",
        &state.env_key,
        "ENV_VAR_NAME or -",
    ));

    let marker = if state.field == ProviderSetupField::WireApi {
        "  › ".cyan()
    } else {
        "    ".into()
    };
    let chat = if state.wire_api == WireApi::Chat {
        " Chat ".cyan().bold()
    } else {
        " Chat ".dim()
    };
    let responses = if state.wire_api == WireApi::Responses {
        " Responses ".cyan().bold()
    } else {
        " Responses ".dim()
    };
    lines.push(Line::from(vec![
        marker,
        "wire type  ".dim(),
        chat,
        " ".into(),
        responses,
    ]));

    let marker = if state.field == ProviderSetupField::FetchModels {
        "  › ".cyan()
    } else {
        "    ".into()
    };
    let label = if state.is_saving {
        "Fetching models..."
    } else {
        "Fetch models"
    };
    lines.push(Line::from(vec![marker, label.green().bold()]));
    lines
}

fn provider_setup_form_row(
    selected: bool,
    label: &'static str,
    value: &str,
    placeholder: &str,
) -> Line<'static> {
    let marker = if selected {
        "  › ".cyan()
    } else {
        "    ".into()
    };
    let value = if value.is_empty() {
        placeholder.to_string().dim()
    } else if selected {
        value.to_string().cyan()
    } else {
        value.to_string().into()
    };
    Line::from(vec![marker, format!("{label:<11}").dim(), value])
}

fn provider_setup_wire_api_lines(selected: WireApi) -> Vec<Line<'static>> {
    [
        (
            WireApi::Chat,
            "Chat Completions",
            "OpenAI-compatible /v1/chat/completions",
        ),
        (WireApi::Responses, "Responses", "OpenAI /v1/responses"),
    ]
    .into_iter()
    .map(|(wire_api, label, description)| {
        if wire_api == selected {
            Line::from(vec![
                "  › ".cyan(),
                label.cyan().bold(),
                " — ".dim(),
                description.cyan(),
            ])
        } else {
            Line::from(vec![
                "    ".into(),
                label.into(),
                " — ".dim(),
                description.dim(),
            ])
        }
    })
    .collect()
}

fn provider_setup_model_lines(state: &ProviderSetupState) -> Vec<Line<'static>> {
    let selected = state.selected_model_index;
    state
        .models
        .iter()
        .enumerate()
        .take(5)
        .map(|(index, model)| {
            let context_window = model.context_window.or(model.max_token_len);
            let context_suffix = context_window
                .map(|window| format!(" — {}K ctx", window / 1024))
                .unwrap_or_default();
            let label = model
                .model_name
                .as_deref()
                .unwrap_or(model.model_id.as_str());
            let display = if label == model.model_id {
                format!("{label}{context_suffix}")
            } else {
                format!("{} ({}){}", label, model.model_id, context_suffix)
            };
            if index == selected {
                Line::from(vec!["  › ".cyan(), display.cyan().bold()])
            } else {
                Line::from(vec!["    ".into(), display.into()])
            }
        })
        .collect()
}

fn provider_setup_field_title(field: ProviderSetupField) -> &'static str {
    match field {
        ProviderSetupField::Id => "Provider id",
        ProviderSetupField::Name => "Display name",
        ProviderSetupField::BaseUrl => "Base URL",
        ProviderSetupField::EnvKey => "API key env var",
        ProviderSetupField::WireApi => "Wire API",
        ProviderSetupField::FetchModels => "Fetch models",
        ProviderSetupField::Model => "Select model",
        ProviderSetupField::ContextWindow => "Context window",
        ProviderSetupField::Confirm => "Confirm",
    }
}

fn provider_setup_placeholder(field: ProviderSetupField) -> &'static str {
    match field {
        ProviderSetupField::Id => "deepseek",
        ProviderSetupField::Name => "DeepSeek",
        ProviderSetupField::BaseUrl => "https://api.deepseek.com/v1",
        ProviderSetupField::EnvKey => "DEEPSEEK_API_KEY or - for no env var",
        ProviderSetupField::WireApi => "chat or responses",
        ProviderSetupField::FetchModels => "",
        ProviderSetupField::Model => "",
        ProviderSetupField::ContextWindow => "262144",
        ProviderSetupField::Confirm => "",
    }
}

pub(super) fn maybe_open_auth_url_in_browser(request_handle: &AppServerRequestHandle, url: &str) {
    if !matches!(request_handle, AppServerRequestHandle::InProcess(_)) {
        return;
    }

    if let Err(err) = webbrowser::open(url) {
        tracing::warn!("failed to open browser for login URL: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_core::config::ConfigBuilder;
    use codex_app_server_client::AppServerRequestHandle;
    use codex_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
    use codex_app_server_client::InProcessAppServerClient;
    use codex_app_server_client::InProcessClientStartArgs;
    use codex_arg0::Arg0DispatchPaths;
    use codex_cloud_config::cloud_config_bundle_loader_for_storage;
    use codex_config::types::AuthCredentialsStoreMode;
    use codex_login::AuthKeyringBackendKind;

    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn widget_forced_chatgpt() -> (AuthModeWidget, TempDir) {
        let codex_home = TempDir::new().unwrap();
        let codex_home_path = codex_home.path().to_path_buf();
        let config = ConfigBuilder::default()
            .codex_home(codex_home_path.clone())
            .build()
            .await
            .unwrap();
        let client = InProcessAppServerClient::start(InProcessClientStartArgs {
            arg0_paths: Arg0DispatchPaths::default(),
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            loader_overrides: Default::default(),
            strict_config: false,
            cloud_config_bundle: cloud_config_bundle_loader_for_storage(
                codex_home_path.clone(),
                /*enable_codex_api_key_env*/ false,
                AuthCredentialsStoreMode::File,
                AuthKeyringBackendKind::default(),
                "https://chatgpt.com/backend-api/".to_string(),
                /*auth_route_config*/ None,
            )
            .await,
            feedback: codex_feedback::CodexFeedback::new(),
            log_db: None,
            state_db: None,
            environment_manager: Arc::new(
                codex_app_server_client::EnvironmentManager::default_for_tests(),
            ),
            config_warnings: Vec::new(),
            session_source: serde_json::from_value(serde_json::json!("cli"))
                .expect("cli session source should deserialize"),
            enable_codex_api_key_env: false,
            client_name: "test".to_string(),
            client_version: "test".to_string(),
            experimental_api: true,
            mcp_server_openai_form_elicitation: false,
            opt_out_notification_methods: Vec::new(),
            channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
        })
        .await
        .unwrap();
        let widget = AuthModeWidget {
            request_frame: FrameRequester::test_dummy(),
            highlighted_mode: SignInOption::ChatGpt,
            error: Arc::new(RwLock::new(None)),
            sign_in_state: Arc::new(RwLock::new(SignInState::PickMode)),
            login_status: LoginStatus::NotAuthenticated,
            app_server_request_handle: AppServerRequestHandle::InProcess(client.request_handle()),
            forced_login_method: Some(ForcedLoginMethod::Chatgpt),
            animations_enabled: true,
            animations_suppressed: std::cell::Cell::new(false),
        };
        (widget, codex_home)
    }

    #[tokio::test]
    async fn api_key_flow_disabled_when_chatgpt_forced() {
        let (mut widget, _tmp) = widget_forced_chatgpt().await;

        widget.start_api_key_entry();

        assert_eq!(
            widget.error_message().as_deref(),
            Some(API_KEY_DISABLED_MESSAGE)
        );
        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::PickMode
        ));
    }

    #[tokio::test]
    async fn saving_api_key_is_blocked_when_chatgpt_forced() {
        let (mut widget, _tmp) = widget_forced_chatgpt().await;

        widget.save_api_key("sk-test".to_string());

        assert_eq!(
            widget.error_message().as_deref(),
            Some(API_KEY_DISABLED_MESSAGE)
        );
        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::PickMode
        ));
        assert_eq!(widget.login_status, LoginStatus::NotAuthenticated);
    }

    #[tokio::test]
    async fn existing_non_oauth_chatgpt_login_counts_as_signed_in() {
        for auth_mode in [AuthMode::ChatgptAuthTokens, AuthMode::PersonalAccessToken] {
            let (mut widget, _tmp) = widget_forced_chatgpt().await;
            widget.login_status = LoginStatus::AuthMode(auth_mode);

            let handled = widget.handle_existing_chatgpt_login();

            assert_eq!(handled, true);
            assert!(matches!(
                &*widget.sign_in_state.read().unwrap(),
                SignInState::ChatGptSuccess
            ));
        }
    }

    #[tokio::test]
    async fn cancel_active_attempt_resets_browser_login_state() {
        let (widget, _tmp) = widget_forced_chatgpt().await;
        *widget.error.write().unwrap() = Some("still logging in".to_string());
        *widget.sign_in_state.write().unwrap() =
            SignInState::ChatGptContinueInBrowser(ContinueInBrowserState {
                login_id: "login-1".to_string(),
                auth_url: "https://auth.example.com".to_string(),
            });

        widget.cancel_active_attempt();

        assert_eq!(widget.error_message(), None);
        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::PickMode
        ));
    }

    #[tokio::test]
    async fn cancel_active_attempt_notifies_device_code_login() {
        let (widget, _tmp) = widget_forced_chatgpt().await;
        *widget.error.write().unwrap() = Some("still logging in".to_string());
        *widget.sign_in_state.write().unwrap() =
            SignInState::ChatGptDeviceCode(ContinueWithDeviceCodeState::ready(
                "request-1".to_string(),
                "login-1".to_string(),
                "https://chatgpt.com/device".to_string(),
                "ABCD-EFGH".to_string(),
            ));

        widget.cancel_active_attempt();

        assert_eq!(widget.error_message(), None);
        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::PickMode
        ));
    }

    /// Collects all buffer cell symbols that contain the OSC 8 open sequence
    /// for the given URL.  Returns the concatenated "inner" characters.
    fn collect_osc8_chars(buf: &Buffer, area: Rect, url: &str) -> String {
        let open = format!("\x1B]8;;{url}\x07");
        let close = "\x1B]8;;\x07";
        let mut chars = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let sym = buf[(x, y)].symbol();
                if let Some(rest) = sym.strip_prefix(open.as_str())
                    && let Some(ch) = rest.strip_suffix(close)
                {
                    chars.push_str(ch);
                }
            }
        }
        chars
    }

    #[test]
    fn continue_in_browser_renders_osc8_hyperlink() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (widget, _tmp) = runtime.block_on(widget_forced_chatgpt());
        let url = "https://auth.example.com/login?state=abc123";
        *widget.sign_in_state.write().unwrap() =
            SignInState::ChatGptContinueInBrowser(ContinueInBrowserState {
                login_id: "login-1".to_string(),
                auth_url: url.to_string(),
            });

        // Render into a narrow buffer so the URL wraps across multiple rows.
        let area = Rect::new(0, 0, 30, 20);
        let mut buf = Buffer::empty(area);
        widget.render_continue_in_browser(area, &mut buf);

        // Every character of the URL should be present as an OSC 8 cell.
        let found = collect_osc8_chars(&buf, area, url);
        assert_eq!(found, url, "OSC 8 hyperlink should cover the full URL");
    }

    #[test]
    fn auth_widget_suppresses_animations_when_device_code_is_visible() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (widget, _tmp) = runtime.block_on(widget_forced_chatgpt());
        *widget.sign_in_state.write().unwrap() =
            SignInState::ChatGptDeviceCode(ContinueWithDeviceCodeState::ready(
                "request-1".to_string(),
                "login-1".to_string(),
                "https://chatgpt.com/device".to_string(),
                "ABCD-EFGH".to_string(),
            ));

        assert_eq!(widget.should_suppress_animations(), true);
    }

    #[test]
    fn auth_widget_suppresses_animations_while_requesting_device_code() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (widget, _tmp) = runtime.block_on(widget_forced_chatgpt());
        *widget.sign_in_state.write().unwrap() = SignInState::ChatGptDeviceCode(
            ContinueWithDeviceCodeState::pending("request-1".to_string()),
        );

        assert_eq!(widget.should_suppress_animations(), true);
    }

    #[tokio::test]
    async fn device_code_login_completion_advances_to_success_message() {
        let (mut widget, _tmp) = widget_forced_chatgpt().await;
        *widget.sign_in_state.write().unwrap() =
            SignInState::ChatGptDeviceCode(ContinueWithDeviceCodeState::ready(
                "request-1".to_string(),
                "login-1".to_string(),
                "https://chatgpt.com/device".to_string(),
                "ABCD-EFGH".to_string(),
            ));

        widget.on_account_login_completed(AccountLoginCompletedNotification {
            login_id: Some("login-1".to_string()),
            success: true,
            error: None,
        });

        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::ChatGptSuccessMessage
        ));
    }

    #[test]
    fn mark_url_hyperlink_wraps_cyan_underlined_cells() {
        let url = "https://example.com";
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);

        // Manually write some cyan+underlined characters to simulate a rendered URL.
        for (i, ch) in "example".chars().enumerate() {
            let cell = &mut buf[(i as u16, 0)];
            cell.set_symbol(&ch.to_string());
            cell.fg = Color::Cyan;
            cell.modifier = Modifier::UNDERLINED;
        }
        // Leave a plain cell that should NOT be marked.
        buf[(7, 0)].set_symbol("X");

        mark_url_hyperlink(&mut buf, area, url);

        // Each cyan+underlined cell should now carry the OSC 8 wrapper.
        let found = collect_osc8_chars(&buf, area, url);
        assert_eq!(found, "example");

        // The plain "X" cell should be untouched.
        assert_eq!(buf[(7, 0)].symbol(), "X");
    }

    #[test]
    fn mark_url_hyperlink_sanitizes_control_chars() {
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);

        // One cyan+underlined cell to mark.
        let cell = &mut buf[(0, 0)];
        cell.set_symbol("a");
        cell.fg = Color::Cyan;
        cell.modifier = Modifier::UNDERLINED;

        // URL contains ESC and BEL that could break the OSC 8 sequence.
        let malicious_url = "https://evil.com/\x1B]8;;\x07injected";
        mark_url_hyperlink(&mut buf, area, malicious_url);

        let sym = buf[(0, 0)].symbol().to_string();
        // The sanitized URL retains `]` (printable) but strips ESC and BEL.
        let sanitized = "https://evil.com/]8;;injected";
        assert!(
            sym.contains(sanitized),
            "symbol should contain sanitized URL, got: {sym:?}"
        );
        // The injected close-sequence must not survive: \x1B and \x07 are gone.
        assert!(
            !sym.contains("\x1B]8;;\x07injected"),
            "symbol must not contain raw control chars from URL"
        );
    }
}
