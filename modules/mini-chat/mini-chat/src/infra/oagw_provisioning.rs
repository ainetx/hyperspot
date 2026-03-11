//! OAGW upstream and route registration for configured LLM providers.
//!
//! All provider upstreams, routes, and tenant-override upstreams are registered
//! as GTS entities in the types-registry during `init()`. OAGW materialises
//! them in `post_init()` via its type-provisioning pipeline.

use std::collections::HashMap;

use tracing::info;
use types_registry_sdk::{RegisterResult, TypesRegistryClient};

use crate::config::ProviderEntry;
use crate::infra::type_catalog;

/// Register provider upstream, route, and tenant-override GTS entities in the types-registry.
///
/// Called during `init()`. OAGW will materialise these in `post_init()`.
pub async fn register_provider_entities(
    registry: &dyn TypesRegistryClient,
    providers: &HashMap<String, ProviderEntry>,
) -> anyhow::Result<()> {
    let tenant_id = modkit_security::constants::DEFAULT_TENANT_ID;
    let entities = type_catalog::build_provider_entities(providers, tenant_id);
    let count = entities.len();

    let results = registry.register(entities).await?;
    RegisterResult::ensure_all_ok(&results)
        .map_err(|e| anyhow::anyhow!("mini-chat provider type registration failed: {e}"))?;

    info!(
        count,
        "Registered mini-chat provider GTS entities in types-registry"
    );
    Ok(())
}
