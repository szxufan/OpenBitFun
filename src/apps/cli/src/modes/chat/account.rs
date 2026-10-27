impl ChatMode {
    fn replace_external_conflict_preferences(
        &mut self,
        preferences: ExternalSourceConflictPreferences,
    ) {
        self.external_source_conflict_choices = preferences.choices;
        self.external_source_conflict_lineage_current_keys = preferences.lineage_current_keys;
        self.external_source_conflicted_candidate_ids = preferences.conflicted_candidate_ids;
    }

    fn open_login_or_account_panel(
        &self,
        chat_view: &mut ChatView,
        chat_state: &mut ChatState,
        rt_handle: &tokio::runtime::Handle,
    ) {
        let snapshot = tokio::task::block_in_place(|| {
            rt_handle.block_on(async {
                let account = self.account_runtime.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("Account management is unavailable for this TUI Host")
                })?;
                Ok::<_, anyhow::Error>(crate::account::account_snapshot_projection(
                    account.snapshot().await,
                ))
            })
        });
        match snapshot {
            Ok(snapshot) if snapshot.logged_in => self.open_account_panel(chat_view, snapshot),
            Ok(_) => {
                // A rejected device token may still have a usable refresh
                // token: try one silent renewal before asking for credentials.
                if let Some(login) = self.try_renew_expired_session(rt_handle) {
                    chat_state
                        .add_system_message(crate::account::account_login_status_message(&login));
                    let renewed = tokio::task::block_in_place(|| {
                        rt_handle.block_on(async {
                            let account = self.account_runtime.as_ref().ok_or_else(|| {
                                anyhow::anyhow!(
                                    "Account management is unavailable for this TUI Host"
                                )
                            })?;
                            Ok::<_, anyhow::Error>(crate::account::account_snapshot_projection(
                                account.snapshot().await,
                            ))
                        })
                    });
                    if let Ok(snapshot) = renewed {
                        self.open_account_panel(chat_view, snapshot);
                        return;
                    }
                }
                chat_view.show_login_form();
            }
            Err(error) => {
                tracing::warn!(
                    "Failed to load account: {}",
                    crate::account::bounded_account_error(&error.to_string())
                );
                chat_view.show_login_form();
                chat_view.login_form_set_error(format!(
                    "Failed to load account: {}",
                    crate::account_guidance::account_failure_line(&error.to_string())
                ));
            }
        }
    }

    /// Session-expiry path: attempt one silent renewal from the persisted
    /// refresh token. `None` means the user must sign in with new credentials.
    fn try_renew_expired_session(
        &self,
        rt_handle: &tokio::runtime::Handle,
    ) -> Option<openbitfun_core::service::remote_connect::account_runtime::AccountLoginResult> {
        let renewal = tokio::task::block_in_place(|| {
            rt_handle.block_on(async {
                let account = self.account_runtime.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("Account management is unavailable for this TUI Host")
                })?;
                if !account.is_token_expired() {
                    return Ok(None);
                }
                account.try_refresh_relogin().await.map(Some)
            })
        });
        match renewal {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(
                    "Session renewal failed: {}",
                    crate::account::bounded_account_error(&error.to_string())
                );
                None
            }
        }
    }

    fn open_account_panel(&self, chat_view: &mut ChatView, snapshot: AccountSnapshotProjection) {
        let Some(info) = snapshot.info else {
            chat_view.show_login_form();
            return;
        };
        chat_view.show_account_panel(info, snapshot.devices);
    }

    fn handle_login_form_action(
        &self,
        action: LoginFormAction,
        chat_view: &mut ChatView,
        chat_state: &mut ChatState,
        rt_handle: &tokio::runtime::Handle,
    ) -> Result<Option<ChatExitReason>> {
        match action {
            LoginFormAction::RequestCode { phone } => {
                let result = tokio::task::block_in_place(|| {
                    rt_handle.block_on(async {
                        let account = self.account_runtime.as_ref().ok_or_else(|| {
                            anyhow::anyhow!("Account management is unavailable for this TUI Host")
                        })?;
                        account.local_sms_send(&phone).await
                    })
                });
                match result {
                    Ok(()) => chat_view.login_form_set_status(&format!(
                        "Verification code sent to {phone}. Enter the code, then press Enter."
                    )),
                    Err(error) => {
                        tracing::warn!(
                            "SMS code request failed: {}",
                            crate::account::bounded_account_error(&error.to_string())
                        );
                        chat_view.login_form_set_error(format!(
                            "Could not send the verification code: {}",
                            crate::account::bounded_account_error(&error.to_string())
                        ));
                    }
                }
            }
            LoginFormAction::LoginWithCode { phone, code } => {
                self.complete_local_login(phone, Some(code), None, chat_view, chat_state, rt_handle)
            }
            LoginFormAction::LoginWithPassword { phone, password } => self.complete_local_login(
                phone,
                None,
                Some(password),
                chat_view,
                chat_state,
                rt_handle,
            ),
            LoginFormAction::Logout => {
                match tokio::task::block_in_place(|| {
                    rt_handle.block_on(async {
                        let account = self.account_runtime.as_ref().ok_or_else(|| {
                            anyhow::anyhow!("Account management is unavailable for this TUI Host")
                        })?;
                        account.logout().await?;
                        Ok::<_, anyhow::Error>(crate::account::account_snapshot_projection(
                            account.snapshot().await,
                        ))
                    })
                }) {
                    Ok(_) => {
                        chat_view.show_login_form();
                        chat_state.add_system_message("Logged out.".to_string());
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Logout failed: {}",
                            crate::account::bounded_account_error(&e.to_string())
                        );
                        chat_view.login_form_set_error(format!(
                            "Logout failed: {}",
                            crate::account_guidance::account_failure_line(&e.to_string())
                        ));
                    }
                }
            }
            LoginFormAction::Cancel => {
                chat_view.set_status(Some("Account panel closed".to_string()));
            }
            LoginFormAction::None => {}
        }
        Ok(None)
    }

    /// Sign in with the collected relay-local credentials and adopt the new
    /// account panel state. Runtime errors (cooldowns, closed registration,
    /// SMS outages) are surfaced verbatim so the user can act on them.
    fn complete_local_login(
        &self,
        phone: String,
        code: Option<String>,
        password: Option<String>,
        chat_view: &mut ChatView,
        chat_state: &mut ChatState,
        rt_handle: &tokio::runtime::Handle,
    ) {
        let result = tokio::task::block_in_place(|| {
            rt_handle.block_on(async {
                let account = self.account_runtime.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("Account management is unavailable for this TUI Host")
                })?;
                account
                    .login_with_local_credentials(&phone, code.as_deref(), password.as_deref())
                    .await
            })
        });
        // Never let a password-bearing transport detail reach logs or the UI.
        let result = result.map_err(
            |error| match password.as_deref().filter(|s| !s.is_empty()) {
                Some(secret) => crate::account::redact_login_error(error, [secret, "", ""]),
                None => error,
            },
        );
        match result {
            Ok(login) => {
                chat_state.add_system_message(crate::account::account_login_status_message(&login));
                self.open_login_or_account_panel(chat_view, chat_state, rt_handle);
            }
            Err(error) => {
                let detail = crate::account::bounded_account_error(&error.to_string());
                tracing::warn!("Login failed: {detail}");
                chat_view.login_form_set_error(format!("Login failed: {detail}"));
            }
        }
    }

    /// Check if any popup is currently visible
    fn any_popup_visible(&self, chat_view: &ChatView) -> bool {
        chat_view.command_palette_visible()
            || chat_view.model_selector_visible()
            || chat_view.agent_selector_visible()
            || chat_view.session_selector_visible()
            || chat_view.session_lineage_selector_visible()
            || chat_view.fork_selector_visible()
            || chat_view.timeline_selector_visible()
            || chat_view.prompt_stash_selector_visible()
            || chat_view.export_dialog_visible()
            || chat_view.skill_selector_visible()
            || chat_view.subagent_selector_visible()
            || chat_view.mcp_selector_visible()
            || chat_view.mcp_add_dialog_visible()
            || chat_view.provider_selector_visible()
            || chat_view.model_config_form_visible()
            || chat_view.login_form_visible()
            || chat_view.theme_selector_visible()
            || chat_view.info_popup_visible()
            || chat_view.prompt_command_shell_review_visible()
            || chat_view.workspace_diff_visible()
    }

    /// Close all popups and clear the navigation stack
    fn close_all_popups(&mut self, chat_view: &mut ChatView) {
        // Cancel theme preview if active
        if chat_view.theme_selector_visible() {
            chat_view.cancel_theme_preview();
        }
        chat_view.hide_command_palette();
        chat_view.hide_model_selector();
        chat_view.hide_agent_selector();
        chat_view.hide_session_selector();
        chat_view.hide_session_lineage_selector();
        chat_view.hide_fork_selector();
        chat_view.hide_timeline_selector();
        chat_view.hide_prompt_stash_selector();
        chat_view.hide_export_dialog();
        chat_view.hide_skill_selector();
        chat_view.hide_subagent_selector();
        chat_view.hide_mcp_selector();
        chat_view.hide_mcp_add_dialog();
        chat_view.hide_provider_selector();
        chat_view.hide_model_config_form();
        chat_view.hide_login_form();
        chat_view.hide_theme_selector();
        chat_view.dismiss_info_popup();
        chat_view.hide_prompt_command_shell_review();
        self.pending_prompt_command_shell_invocation = None;
        chat_view.hide_workspace_diff();
        chat_view.popup_stack.clear();
    }

    /// Navigate back to the previous popup in the stack, or close all if at the root
    fn navigate_back(&self, chat_view: &mut ChatView) {
        // Pop the current popup from the stack and hide it
        if let Some(current) = chat_view.popup_stack.pop() {
            // Hide the current popup
            match current {
                crate::ui::chat::PopupType::CommandPalette => chat_view.hide_command_palette(),
                crate::ui::chat::PopupType::ModelSelector => chat_view.hide_model_selector(),
                crate::ui::chat::PopupType::AgentSelector => chat_view.hide_agent_selector(),
                crate::ui::chat::PopupType::SessionSelector => chat_view.hide_session_selector(),
                crate::ui::chat::PopupType::SessionLineageSelector => {
                    chat_view.hide_session_lineage_selector()
                }
                crate::ui::chat::PopupType::ForkSelector => chat_view.hide_fork_selector(),
                crate::ui::chat::PopupType::TimelineSelector => chat_view.hide_timeline_selector(),
                crate::ui::chat::PopupType::PromptStashSelector => {
                    chat_view.hide_prompt_stash_selector()
                }
                crate::ui::chat::PopupType::ExportDialog => chat_view.hide_export_dialog(),
                crate::ui::chat::PopupType::SkillSelector => chat_view.hide_skill_selector(),
                crate::ui::chat::PopupType::SubagentSelector => chat_view.hide_subagent_selector(),
                crate::ui::chat::PopupType::McpSelector => chat_view.hide_mcp_selector(),
                crate::ui::chat::PopupType::McpAddDialog => chat_view.hide_mcp_add_dialog(),
                crate::ui::chat::PopupType::ProviderSelector => chat_view.hide_provider_selector(),
                crate::ui::chat::PopupType::ModelConfigForm => chat_view.hide_model_config_form(),
                crate::ui::chat::PopupType::LoginForm => chat_view.hide_login_form(),
                crate::ui::chat::PopupType::ThemeSelector => {
                    chat_view.hide_theme_selector();
                    chat_view.cancel_theme_preview();
                }
                crate::ui::chat::PopupType::InfoPopup => chat_view.dismiss_info_popup(),
                crate::ui::chat::PopupType::WorkspaceDiff => chat_view.hide_workspace_diff(),
            }

            // If there's a previous popup in the stack, re-show it
            if let Some(previous) = chat_view.popup_stack.peek() {
                match previous {
                    crate::ui::chat::PopupType::CommandPalette => {
                        chat_view.reshow_command_palette()
                    }
                    crate::ui::chat::PopupType::ModelSelector => chat_view.reshow_model_selector(),
                    crate::ui::chat::PopupType::AgentSelector => chat_view.reshow_agent_selector(),
                    crate::ui::chat::PopupType::SessionSelector => {
                        chat_view.reshow_session_selector()
                    }
                    crate::ui::chat::PopupType::SessionLineageSelector => {
                        chat_view.reshow_session_lineage_selector()
                    }
                    crate::ui::chat::PopupType::ForkSelector => chat_view.reshow_fork_selector(),
                    crate::ui::chat::PopupType::TimelineSelector => {
                        chat_view.reshow_timeline_selector()
                    }
                    crate::ui::chat::PopupType::PromptStashSelector => {
                        chat_view.reshow_prompt_stash_selector()
                    }
                    crate::ui::chat::PopupType::ExportDialog => {}
                    crate::ui::chat::PopupType::SkillSelector => chat_view.reshow_skill_selector(),
                    crate::ui::chat::PopupType::SubagentSelector => {
                        chat_view.reshow_subagent_selector()
                    }
                    crate::ui::chat::PopupType::McpSelector => chat_view.reshow_mcp_selector(),
                    crate::ui::chat::PopupType::McpAddDialog => chat_view.reshow_mcp_add_dialog(),
                    crate::ui::chat::PopupType::ProviderSelector => {
                        chat_view.reshow_provider_selector()
                    }
                    crate::ui::chat::PopupType::ModelConfigForm => {
                        chat_view.reshow_model_config_form()
                    }
                    crate::ui::chat::PopupType::LoginForm => chat_view.reshow_login_form(),
                    crate::ui::chat::PopupType::ThemeSelector => chat_view.reshow_theme_selector(),
                    crate::ui::chat::PopupType::InfoPopup => {}
                    crate::ui::chat::PopupType::WorkspaceDiff => chat_view.reshow_workspace_diff(),
                }
            }
        }
    }
}
