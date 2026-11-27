use agent_client_protocol::schema::AgentCapabilities;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AcpRemoteSessionStrategy {
    New,
    Load,
    Resume,
}

impl AcpRemoteSessionStrategy {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Load => "load",
            Self::Resume => "resume",
        }
    }

    pub(super) fn startup_phase_name(self) -> &'static str {
        match self {
            Self::New => "session creation",
            Self::Load => "session/load",
            Self::Resume => "session/resume",
        }
    }
}

pub(super) fn preferred_resume_strategies(
    capabilities: Option<&AgentCapabilities>,
    remote_session_id: Option<&str>,
    require_restore: bool,
) -> Vec<AcpRemoteSessionStrategy> {
    use AcpRemoteSessionStrategy::{Load, New, Resume};

    if !remote_session_id.is_some_and(|id| !id.trim().is_empty()) {
        return if require_restore { vec![] } else { vec![New] };
    }
    let can_resume = capabilities.is_some_and(|caps| caps.session_capabilities.resume.is_some());
    let can_load = capabilities.is_some_and(|caps| caps.load_session);
    // Resume does not tell us whether an unconfirmed prompt can still emit
    // output. Load the saved session before another prompt takes its receiver.
    if require_restore {
        return if can_load { vec![Load] } else { Vec::new() };
    }
    let mut strategies = Vec::new();
    if can_resume {
        strategies.push(Resume);
    }
    if can_load {
        strategies.push(Load);
    }
    strategies
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::{SessionCapabilities, SessionResumeCapabilities};
    use AcpRemoteSessionStrategy::{Load, New, Resume};

    #[test]
    fn falls_back_to_new_without_remote_session_id() {
        assert_eq!(preferred_resume_strategies(None, None, false), vec![New]);
        assert!(preferred_resume_strategies(None, Some(" "), true).is_empty());
    }

    #[test]
    fn restores_existing_sessions_with_the_negotiated_agent() {
        let strategies = |caps: &AgentCapabilities, required| {
            preferred_resume_strategies(Some(caps), Some("original"), required)
        };
        let load_only = AgentCapabilities::new().load_session(true);
        let both = load_only.clone().session_capabilities(
            SessionCapabilities::new().resume(SessionResumeCapabilities::new()),
        );
        let resume_only = both.clone().load_session(false);
        assert_eq!(strategies(&both, false), vec![Resume, Load]);
        assert_eq!(strategies(&both, true), vec![Load]);
        assert_eq!(strategies(&load_only, false), vec![Load]);
        assert!(strategies(&resume_only, true).is_empty());
        assert!(strategies(&AgentCapabilities::new(), false).is_empty());
    }
}
