//! Pure account-pool validation and monotonic policy revisions.
use crate::{AIProviderConfig, AuthConfig};
use std::collections::HashSet;

pub fn validate_account_pool(auth: &AuthConfig) -> Result<(), String> {
    let AuthConfig::SubscriptionPool { accounts, .. } = auth else {
        return Ok(());
    };
    let mut seen = HashSet::new();
    if accounts.is_empty()
        || accounts
            .iter()
            .any(|id| id.trim().is_empty() || id.trim() != id || !seen.insert(id))
    {
        return Err("Subscription account pool must contain distinct non-empty account IDs".into());
    }
    Ok(())
}

/// Normal writes reject stale policy revisions. Explicit imports/restores may
/// rebase them onto the current generation without reusing an old policy epoch.
/// Unchanged historical records are preserved, including unsupported shapes.
pub fn reconcile_account_pool_revisions(
    previous: &[AIProviderConfig],
    next: &mut [AIProviderConfig],
    reject_stale: bool,
) -> Result<(), String> {
    for provider in next {
        let mut matches = previous.iter().filter(|old| old.id == provider.id);
        let old = matches.next();
        if !matches!(provider.auth, AuthConfig::SubscriptionPool { .. }) {
            continue;
        }
        if matches.next().is_some() {
            return Err(format!(
                "Ambiguous account pool provider ID: {}",
                provider.id
            ));
        }
        if old.is_some_and(|old| old.auth == provider.auth) {
            continue;
        }
        validate_account_pool(&provider.auth)?;
        if let (
            Some(AIProviderConfig {
                auth:
                    AuthConfig::SubscriptionPool {
                        provider: old_pool_provider,
                        accounts: old_accounts,
                        revision: old_revision,
                    },
                ..
            }),
            AuthConfig::SubscriptionPool {
                provider: pool_provider,
                accounts,
                revision,
            },
        ) = (old, &mut provider.auth)
        {
            if reject_stale && *revision < *old_revision {
                return Err(format!(
                    "SUBSCRIPTION_ACCOUNT_POOL_CONFLICT: Subscription account pool changed while it was being edited: {}",
                    provider.id
                ));
            }
            if pool_provider != old_pool_provider || accounts != old_accounts {
                let minimum = old_revision.checked_add(1).ok_or_else(|| "Subscription account pool revision overflow; existing configuration was preserved".to_string())?;
                *revision = (*revision).max(minimum);
            } else {
                *revision = (*revision).max(*old_revision);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SubscriptionProvider;
    fn model(accounts: &[&str], revision: u64) -> AIProviderConfig {
        AIProviderConfig {
            id: "one".into(),
            auth: AuthConfig::SubscriptionPool {
                provider: SubscriptionProvider::Codex,
                accounts: accounts.iter().map(|id| (*id).into()).collect(),
                revision,
            },
            ..Default::default()
        }
    }
    #[test]
    fn every_registered_provider_pool_round_trips_without_changing_legacy_defaults() {
        for provider in [
            SubscriptionProvider::Codex,
            SubscriptionProvider::Antigravity,
            SubscriptionProvider::Opencode,
            SubscriptionProvider::Grok,
            SubscriptionProvider::Hermes,
        ] {
            let auth = AuthConfig::SubscriptionPool {
                provider,
                accounts: vec![
                    provider.key().into(),
                    format!(
                        "{}-account-{}",
                        provider.key(),
                        "58134808-15a6-45db-ae33-2ad7d85cc570"
                    ),
                ],
                revision: 2,
            };
            validate_account_pool(&auth).unwrap();
            let payload = serde_json::to_value(&auth).unwrap();
            assert_eq!(serde_json::from_value::<AuthConfig>(payload).unwrap(), auth);
            let legacy = serde_json::json!({"type":"subscription","provider":provider.key()});
            assert_eq!(
                serde_json::to_value(serde_json::from_value::<AuthConfig>(legacy.clone()).unwrap())
                    .unwrap(),
                legacy
            );
        }
    }
    #[test]
    fn order_and_membership_advance_but_other_edits_keep_the_policy_epoch() {
        let old = model(&["codex", "backup"], 0);
        let mut next = vec![model(&["backup", "codex"], 0)];
        reconcile_account_pool_revisions(&[old.clone()], &mut next, true).unwrap();
        assert!(matches!(
            next[0].auth,
            AuthConfig::SubscriptionPool { revision: 1, .. }
        ));
        let old = next[0].clone();
        next[0].name = "Renamed".into();
        reconcile_account_pool_revisions(&[old], &mut next, true).unwrap();
        assert!(matches!(
            next[0].auth,
            AuthConfig::SubscriptionPool { revision: 1, .. }
        ));
    }
    #[test]
    fn stale_writes_fail_and_explicit_restores_rebase_instead_of_reusing_old_epochs() {
        let old = model(&["codex", "backup"], 7);
        let mut next = vec![model(&["backup", "codex"], 2)];
        assert!(reconcile_account_pool_revisions(&[old.clone()], &mut next, true).is_err());
        assert!(matches!(
            next[0].auth,
            AuthConfig::SubscriptionPool { revision: 2, .. }
        ));
        reconcile_account_pool_revisions(&[old], &mut next, false).unwrap();
        assert!(matches!(
            next[0].auth,
            AuthConfig::SubscriptionPool { revision: 8, .. }
        ));
    }
    #[test]
    fn invalid_new_pools_and_overflow_fail_without_resetting_legacy_records() {
        for accounts in [vec![], vec!["codex", "codex"], vec![""]] {
            let mut next = vec![model(&accounts, 0)];
            assert!(reconcile_account_pool_revisions(&[], &mut next, true).is_err());
            let legacy = next.clone();
            reconcile_account_pool_revisions(&legacy, &mut next, true).unwrap();
        }
        let mut next = vec![model(&["backup"], u64::MAX)];
        assert!(
            reconcile_account_pool_revisions(&[model(&["codex"], u64::MAX)], &mut next, true)
                .is_err()
        );
    }
}
