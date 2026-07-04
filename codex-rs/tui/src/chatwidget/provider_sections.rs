//! Provider manager grouping and copy helpers.

use codex_model_provider_info::LMSTUDIO_OSS_PROVIDER_ID;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::OLLAMA_OSS_PROVIDER_ID;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProviderListSection {
    ManagedBuiltIn,
    LocalOss,
    Custom,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ProviderSectionCounts {
    pub(super) managed: usize,
    pub(super) local_oss: usize,
    pub(super) custom: usize,
}

pub(super) fn provider_section_counts(
    providers: &[(String, ModelProviderInfo, bool, ProviderListSection)],
) -> ProviderSectionCounts {
    let mut counts = ProviderSectionCounts::default();
    for (_, _, _, section) in providers {
        match section {
            ProviderListSection::ManagedBuiltIn => counts.managed += 1,
            ProviderListSection::LocalOss => counts.local_oss += 1,
            ProviderListSection::Custom => counts.custom += 1,
        }
    }
    counts
}

pub(super) fn provider_list_section(id: &str, is_builtin: bool) -> ProviderListSection {
    if !is_builtin {
        return ProviderListSection::Custom;
    }

    match id {
        OLLAMA_OSS_PROVIDER_ID | LMSTUDIO_OSS_PROVIDER_ID => ProviderListSection::LocalOss,
        _ => ProviderListSection::ManagedBuiltIn,
    }
}

pub(super) fn provider_fetch_models_description(
    section: ProviderListSection,
    provider: &ModelProviderInfo,
) -> String {
    match section {
        ProviderListSection::ManagedBuiltIn => {
            "Managed providers use Codex's built-in model catalog".to_string()
        }
        ProviderListSection::LocalOss => {
            "Local OSS models are managed by local-model setup flows".to_string()
        }
        ProviderListSection::Custom if provider.models.is_empty() => {
            "Fetch this provider's models, cache them, then choose one".to_string()
        }
        ProviderListSection::Custom => {
            format!(
                "Refresh {} cached models, then choose one",
                provider.models.len()
            )
        }
    }
}

pub(super) fn provider_description(
    id: &str,
    provider: &ModelProviderInfo,
    is_builtin: bool,
) -> String {
    let source = match provider_list_section(id, is_builtin) {
        ProviderListSection::ManagedBuiltIn => "managed",
        ProviderListSection::LocalOss => "local oss",
        ProviderListSection::Custom => "custom",
    };
    let base_url = provider.base_url.as_deref().unwrap_or("no base_url");
    let env_key = provider.env_key.as_deref().unwrap_or("no env_key");
    format!("{source} - {} - {base_url} - {env_key}", provider.wire_api)
}
