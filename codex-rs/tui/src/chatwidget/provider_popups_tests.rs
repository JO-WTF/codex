use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc::unbounded_channel;

use super::*;

fn provider_form_view() -> (
    ProviderFormView,
    tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) {
    let (tx, rx) = unbounded_channel();
    let draft = ProviderFormDraft {
        id: "deepseek".to_string(),
        name: "DeepSeek".to_string(),
        base_url: "https://api.deepseek.com/v1".to_string(),
        env_key: "DEEPSEEK_API_KEY".to_string(),
        wire_api: WireApi::Chat,
    };
    (
        ProviderFormView::new(ProviderFormMode::Add, draft, AppEventSender::new(tx)),
        rx,
    )
}

#[test]
fn provider_form_tab_moves_to_next_field() {
    let (mut view, _rx) = provider_form_view();

    assert_eq!(view.selected_row(), ProviderFormRow::Id);
    view.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(view.selected_row(), ProviderFormRow::Name);
}

#[test]
fn provider_form_space_fetches_models_on_fetch_row() {
    let (mut view, mut rx) = provider_form_view();
    let Some(fetch_models_row) = ProviderFormRow::rows(ProviderFormMode::Add)
        .iter()
        .position(|row| *row == ProviderFormRow::FetchModels)
    else {
        panic!("fetch models row exists");
    };
    view.selected = fetch_models_row;

    view.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert!(view.is_fetching);
    let event = match rx.try_recv() {
        Ok(event) => event,
        Err(err) => panic!("provider config action: {err}"),
    };
    match event {
        AppEvent::ProviderConfigAction {
            action:
                crate::app_event::ProviderConfigAction::FetchModelsForNewProvider { draft, provider },
        } => {
            assert_eq!(draft.id, "deepseek");
            assert_eq!(provider.name, "DeepSeek");
        }
        event => panic!("unexpected event: {event:?}"),
    }
}
