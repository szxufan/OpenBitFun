use super::*;

const DISABLED_MESSAGE: &str = "Subagent model overrides are disabled by the user. Remove model_id and retry to use the configured subagent model.";

/// Execution-host policy. Workspace and controller-provided context cannot override it.
#[derive(Clone, Copy)]
pub(super) struct ModelOverridePolicy {
    pub(super) allowed: bool,
}

impl ModelOverridePolicy {
    pub(super) async fn presentation() -> OpenBitFunResult<Self> {
        match MODEL_PRESENTATION_POLICY.try_with(|policy| *policy) {
            Ok(policy) => Ok(policy),
            Err(_) => Self::load().await,
        }
    }

    pub(super) async fn load() -> OpenBitFunResult<Self> {
        #[cfg(test)]
        {
            if let Ok(allowed) = TEST_MODEL_OVERRIDE.try_with(|allowed| *allowed) {
                return Ok(Self { allowed });
            }
            if !GlobalConfigManager::is_initialized() {
                return Ok(Self { allowed: true });
            }
        }
        let service = GlobalConfigManager::get_service().await?;
        let allowed = service
            .get_config(Some("ai.allow_subagent_model_override"))
            .await?;
        Ok(Self { allowed })
    }

    pub(super) fn schema(self, mut schema: Value) -> Value {
        if !self.allowed {
            if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
                properties.remove("model_id");
            }
        }
        schema
    }

    pub(super) async fn model_schema(schema: Value) -> Value {
        let policy = Self::presentation().await.unwrap_or_else(|error| {
            warn!("Failed to read subagent model override policy: {}", error);
            Self { allowed: false }
        });
        policy.schema(schema)
    }

    pub(super) fn check(self, input: &Value) -> OpenBitFunResult<()> {
        if !self.allowed && input.get("model_id").is_some() {
            return Err(OpenBitFunError::validation(DISABLED_MESSAGE.to_string()));
        }
        Ok(())
    }

    /// Check original arguments before translation can drop null/empty fields.
    pub(super) async fn enforce(input: &Value) -> OpenBitFunResult<()> {
        if input.get("model_id").is_some() {
            Self::load().await?.check(input)?;
        }
        Ok(())
    }
}

tokio::task_local! {
    static MODEL_PRESENTATION_POLICY: ModelOverridePolicy;
}

/// Freeze only tool presentation during one catalog request. Execution always
/// re-reads the host policy so an already-issued tool call cannot bypass an opt-out.
pub(crate) fn with_collaboration_model_policy<F: std::future::Future>(
    future: F,
) -> impl std::future::Future<Output = F::Output> {
    // Box before constructing the async state machine. An async fn would still
    // retain its inline F argument, and nested catalog scopes multiply both its
    // size and TaskLocalFuture's debug poll temporaries (especially with PTC).
    let future = Box::pin(future);
    async move {
        if MODEL_PRESENTATION_POLICY.try_with(|_| ()).is_ok() {
            return future.await;
        }
        let policy = ModelOverridePolicy::load().await.unwrap_or_else(|error| {
            warn!(
                "Failed to read collaboration presentation policy: {}",
                error
            );
            ModelOverridePolicy { allowed: false }
        });
        crate::agentic::tools::registry::get_global_tool_registry()
            .write()
            .await
            .refresh_collaboration_model_policy(policy.allowed);
        MODEL_PRESENTATION_POLICY.scope(policy, future).await
    }
}

pub(super) async fn model_override_validation(input: &Value) -> Option<ValidationResult> {
    ModelOverridePolicy::enforce(input)
        .await
        .err()
        .map(|error| AgentExecutionTool::invalid_input(error.to_string()))
}

#[cfg(test)]
tokio::task_local! {
    pub(super) static TEST_MODEL_OVERRIDE: bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;

    #[test]
    fn collaboration_policy_future_size_does_not_grow_with_its_input() {
        async fn input<const N: usize>() {
            let buffer = [0u8; N];
            std::future::pending::<()>().await;
            std::hint::black_box(buffer);
        }

        fn future_size<F: Future>(_: impl FnOnce() -> F) -> usize {
            std::mem::size_of::<F>()
        }

        // Infer sizes without allocating the large fixture on the test stack.
        assert!(future_size(input::<65536>) > future_size(input::<1>));
        assert_eq!(
            future_size(|| with_collaboration_model_policy(input::<1>())),
            future_size(|| with_collaboration_model_policy(input::<65536>())),
            "The policy wrapper must not embed the catalog future's state"
        );
        assert_eq!(
            future_size(|| with_collaboration_model_policy(input::<65536>())),
            future_size(|| {
                with_collaboration_model_policy(with_collaboration_model_policy(input::<65536>()))
            }),
            "Nested catalog scopes must not multiply the wrapper's state"
        );
    }

    #[tokio::test]
    async fn collaboration_policy_remains_lazy_inherited_and_scoped() {
        let wrapped = with_collaboration_model_policy(async {
            assert!(!ModelOverridePolicy::presentation().await.unwrap().allowed);
            tokio::task::yield_now().await;
            assert!(!ModelOverridePolicy::presentation().await.unwrap().allowed);
            TEST_MODEL_OVERRIDE
                .scope(
                    true,
                    with_collaboration_model_policy(async {
                        // Nested presentation inherits the snapshot, while execution
                        // policy still reads the current host setting independently.
                        assert!(!ModelOverridePolicy::presentation().await.unwrap().allowed);
                        assert!(ModelOverridePolicy::load().await.unwrap().allowed);
                    }),
                )
                .await;
        });
        assert!(MODEL_PRESENTATION_POLICY.try_with(|_| ()).is_err());
        // Select the policy only when polled, after construction outside this scope.
        TEST_MODEL_OVERRIDE.scope(false, wrapped).await;
        assert!(MODEL_PRESENTATION_POLICY.try_with(|_| ()).is_err());
    }

    #[tokio::test]
    async fn collaboration_policy_pending_and_dropped_future_does_not_leak_scope() {
        TEST_MODEL_OVERRIDE
            .scope(false, async {
                let mut wrapped = Box::pin(with_collaboration_model_policy(async {
                    assert!(!ModelOverridePolicy::presentation().await.unwrap().allowed);
                    std::future::pending::<()>().await;
                }));
                std::future::poll_fn(|cx| {
                    assert!(wrapped.as_mut().poll(cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
                assert!(MODEL_PRESENTATION_POLICY.try_with(|_| ()).is_err());
                drop(wrapped);
                assert!(MODEL_PRESENTATION_POLICY.try_with(|_| ()).is_err());
            })
            .await;
    }
}
