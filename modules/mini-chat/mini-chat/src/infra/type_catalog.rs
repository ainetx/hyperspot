//! GTS entity builders for registering mini-chat provider upstreams and routes
//! in the types-registry.
//!
//! Entities follow the format expected by OAGW's `TypeProvisioningServiceImpl`
//! so that OAGW materialises them during `post_init()`.

use std::collections::HashMap;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::config::ProviderEntry;

/// UUID v5 namespace for mini-chat deterministic IDs.
const MINI_CHAT_NS: Uuid = Uuid::from_bytes([
    0x6d, 0x69, 0x6e, 0x69, // "mini"
    0x2d, 0x63, // "-c"
    0x68, 0x61, // "ha"
    0x74, 0x2d, // "t-"
    0x6f, 0x61, 0x67, 0x77, // "oagw"
    0x2d, 0x76, // "-v"
]);

/// Build upstream and route GTS entities for all configured providers,
/// including tenant-override upstreams.
///
/// Each provider produces:
/// - one `gts.x.core.oagw.upstream.v1~<uuid>` instance (root upstream)
/// - one `gts.x.core.oagw.route.v1~<uuid>` instance (route match rules)
/// - one upstream per tenant override (with tenant-specific host/auth)
pub fn build_provider_entities(
    providers: &HashMap<String, ProviderEntry>,
    default_tenant_id: Uuid,
) -> Vec<Value> {
    let mut entities = Vec::new();
    for (provider_id, entry) in providers {
        entities.push(build_upstream_entity(provider_id, entry, default_tenant_id));
        entities.push(build_route_entity(provider_id, entry, default_tenant_id));

        // Tenant-override upstreams.
        for tid_str in entry.tenant_overrides.keys() {
            let tenant_id = Uuid::parse_str(tid_str).unwrap_or(default_tenant_id);
            let label = format!("{provider_id}@{tid_str}");
            entities.push(build_tenant_upstream_entity(
                &label, entry, tid_str, tenant_id,
            ));
        }
    }
    entities
}

/// Deterministic UUID for a provider's upstream GTS entity.
#[must_use]
pub fn upstream_uuid(provider_id: &str, tenant_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &MINI_CHAT_NS,
        format!("upstream:{tenant_id}:{provider_id}").as_bytes(),
    )
}

/// Deterministic UUID for a provider's route GTS entity.
#[must_use]
pub fn route_uuid(provider_id: &str, tenant_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &MINI_CHAT_NS,
        format!("route:{tenant_id}:{provider_id}").as_bytes(),
    )
}

fn build_upstream_entity(provider_id: &str, entry: &ProviderEntry, tenant_id: Uuid) -> Value {
    let id = upstream_uuid(provider_id, tenant_id);
    let gts_id = format!("gts.x.core.oagw.upstream.v1~{}", id.hyphenated());

    let mut content = json!({
        "$id": gts_id,
        "tenant_id": tenant_id,
        "server": {
            "endpoints": [{
                "scheme": "https",
                "host": entry.host,
                "port": 443
            }]
        },
        "protocol": "gts.x.core.oagw.protocol.v1~x.core.oagw.http.v1",
        "enabled": true,
        "tags": []
    });

    // Only pass alias when explicitly configured (IP-based hosts).
    if let Some(alias) = &entry.upstream_alias {
        content["alias"] = json!(alias);
    }

    if let (Some(plugin_type), Some(config)) = (&entry.auth_plugin_type, &entry.auth_config) {
        content["auth"] = json!({
            "type": plugin_type,
            "sharing": "inherit",
            "config": config
        });
    }

    content
}

fn build_tenant_upstream_entity(
    label: &str,
    entry: &ProviderEntry,
    tenant_id_str: &str,
    tenant_id: Uuid,
) -> Value {
    let id = upstream_uuid(label, tenant_id);
    let gts_id = format!("gts.x.core.oagw.upstream.v1~{}", id.hyphenated());

    let host = entry.effective_host_for_tenant(tenant_id_str);

    let mut content = json!({
        "$id": gts_id,
        "tenant_id": tenant_id,
        "server": {
            "endpoints": [{
                "scheme": "https",
                "host": host,
                "port": 443
            }]
        },
        "protocol": "gts.x.core.oagw.protocol.v1~x.core.oagw.http.v1",
        "enabled": true,
        "tags": []
    });

    // Only pass alias when the tenant override explicitly sets one.
    if let Some(alias) = entry
        .tenant_overrides
        .get(tenant_id_str)
        .and_then(|o| o.upstream_alias.as_deref())
    {
        content["alias"] = json!(alias);
    }

    if let (Some(plugin_type), Some(config)) = (
        entry.effective_auth_plugin_type_for_tenant(tenant_id_str),
        entry.effective_auth_config_for_tenant(tenant_id_str),
    ) {
        content["auth"] = json!({
            "type": plugin_type,
            "sharing": "inherit",
            "config": config
        });
    }

    content
}

fn build_route_entity(provider_id: &str, entry: &ProviderEntry, tenant_id: Uuid) -> Value {
    let route_id = route_uuid(provider_id, tenant_id);
    let upstream_id = upstream_uuid(provider_id, tenant_id);
    let gts_id = format!("gts.x.core.oagw.route.v1~{}", route_id.hyphenated());

    let (route_prefix, suffix_mode) = derive_route_match(&entry.api_path);
    let query_allowlist = extract_query_allowlist(&entry.api_path);

    let suffix_mode_str = match suffix_mode {
        SuffixMode::Disabled => "disabled",
        SuffixMode::Append => "append",
    };

    json!({
        "$id": gts_id,
        "tenant_id": tenant_id,
        "upstream_id": upstream_id,
        "match": {
            "http": {
                "methods": ["POST"],
                "path": route_prefix,
                "query_allowlist": query_allowlist,
                "path_suffix_mode": suffix_mode_str
            }
        },
        "enabled": true,
        "tags": [],
        "priority": 0
    })
}

// -- Route-match helpers (shared with oagw_provisioning) --

enum SuffixMode {
    Disabled,
    Append,
}

/// Derive route prefix and suffix mode from an `api_path` template.
fn derive_route_match(api_path: &str) -> (String, SuffixMode) {
    let route_path = api_path
        .split('?')
        .next()
        .unwrap_or(api_path)
        .replace("{model}", "*");

    let route_prefix = if let Some(pos) = route_path.find('*') {
        route_path[..pos].trim_end_matches('/').to_owned()
    } else {
        route_path.clone()
    };

    let suffix_mode = if route_path.contains('*') {
        SuffixMode::Append
    } else {
        SuffixMode::Disabled
    };

    (route_prefix, suffix_mode)
}

/// Extract query parameter names from an `api_path` template's query string.
fn extract_query_allowlist(api_path: &str) -> Vec<String> {
    api_path
        .split('?')
        .nth(1)
        .map(|qs| {
            qs.split('&')
                .filter_map(|pair| pair.split('=').next().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::llm::ProviderKind;

    fn test_entry() -> ProviderEntry {
        ProviderEntry {
            kind: ProviderKind::OpenAiResponses,
            upstream_alias: None,
            host: "api.openai.com".to_owned(),
            api_path: "/v1/responses".to_owned(),
            auth_plugin_type: Some(
                "gts.x.core.oagw.auth_plugin.v1~x.core.oagw.apikey.v1".to_owned(),
            ),
            auth_config: Some({
                let mut c = HashMap::new();
                c.insert("header".to_owned(), "Authorization".to_owned());
                c.insert("prefix".to_owned(), "Bearer ".to_owned());
                c.insert("secret_ref".to_owned(), "openai-key".to_owned());
                c
            }),
            tenant_overrides: HashMap::new(),
        }
    }

    #[test]
    fn upstream_uuid_is_deterministic() {
        assert_eq!(
            upstream_uuid("openai", Uuid::nil()),
            upstream_uuid("openai", Uuid::nil())
        );
    }

    #[test]
    fn upstream_uuid_differs_per_provider() {
        assert_ne!(
            upstream_uuid("openai", Uuid::nil()),
            upstream_uuid("azure_openai", Uuid::nil())
        );
    }

    #[test]
    fn route_uuid_is_deterministic() {
        assert_eq!(
            route_uuid("openai", Uuid::nil()),
            route_uuid("openai", Uuid::nil())
        );
    }

    #[test]
    fn route_uuid_differs_from_upstream() {
        assert_ne!(
            upstream_uuid("openai", Uuid::nil()),
            route_uuid("openai", Uuid::nil())
        );
    }

    #[test]
    fn build_upstream_entity_has_correct_gts_id() {
        let entity = build_upstream_entity("openai", &test_entry(), Uuid::nil());
        let id = entity["$id"].as_str().unwrap();
        assert!(id.starts_with("gts.x.core.oagw.upstream.v1~"));
    }

    #[test]
    fn build_upstream_entity_has_auth() {
        let entity = build_upstream_entity("openai", &test_entry(), Uuid::nil());
        assert!(entity["auth"].is_object());
        assert_eq!(
            entity["auth"]["type"],
            "gts.x.core.oagw.auth_plugin.v1~x.core.oagw.apikey.v1"
        );
        assert_eq!(entity["auth"]["sharing"], "inherit");
    }

    #[test]
    fn build_upstream_entity_omits_alias_when_none() {
        let entity = build_upstream_entity("openai", &test_entry(), Uuid::nil());
        assert!(entity.get("alias").is_none());
    }

    #[test]
    fn build_upstream_entity_includes_alias_when_set() {
        let mut entry = test_entry();
        entry.upstream_alias = Some("my-alias".to_owned());
        let entity = build_upstream_entity("openai", &entry, Uuid::nil());
        assert_eq!(entity["alias"], "my-alias");
    }

    #[test]
    fn build_route_entity_has_correct_gts_id() {
        let entity = build_route_entity("openai", &test_entry(), Uuid::nil());
        let id = entity["$id"].as_str().unwrap();
        assert!(id.starts_with("gts.x.core.oagw.route.v1~"));
    }

    #[test]
    fn build_route_entity_references_upstream_uuid() {
        let entity = build_route_entity("openai", &test_entry(), Uuid::nil());
        let upstream_id: Uuid = serde_json::from_value(entity["upstream_id"].clone()).unwrap();
        assert_eq!(upstream_id, upstream_uuid("openai", Uuid::nil()));
    }

    #[test]
    fn build_route_entity_simple_path() {
        let entity = build_route_entity("openai", &test_entry(), Uuid::nil());
        assert_eq!(entity["match"]["http"]["path"], "/v1/responses");
        assert_eq!(entity["match"]["http"]["path_suffix_mode"], "disabled");
    }

    #[test]
    fn build_route_entity_azure_path() {
        let mut entry = test_entry();
        entry.api_path = "/openai/deployments/{model}/responses?api-version=2025-03-01".to_owned();
        let entity = build_route_entity("azure", &entry, Uuid::nil());
        assert_eq!(entity["match"]["http"]["path"], "/openai/deployments");
        assert_eq!(entity["match"]["http"]["path_suffix_mode"], "append");
        assert_eq!(entity["match"]["http"]["query_allowlist"][0], "api-version");
    }

    #[test]
    fn build_provider_entities_returns_two_per_provider() {
        let mut providers = HashMap::new();
        providers.insert("openai".to_owned(), test_entry());

        let entities = build_provider_entities(&providers, Uuid::nil());
        assert_eq!(entities.len(), 2);
    }

    #[test]
    fn build_upstream_entity_no_auth_when_not_configured() {
        let mut entry = test_entry();
        entry.auth_plugin_type = None;
        entry.auth_config = None;
        let entity = build_upstream_entity("openai", &entry, Uuid::nil());
        assert!(entity.get("auth").is_none());
    }
}
