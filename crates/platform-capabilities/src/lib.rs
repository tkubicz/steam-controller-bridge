//! Platform-neutral capability requirements and interactive request flow.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use desktop_input::DesktopSession;

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(any(target_os = "macos", test))]
mod macos;

#[cfg(any(target_os = "linux", test))]
pub use linux::LinuxCapabilities;
#[cfg(any(target_os = "macos", test))]
pub use macos::MacOsCapabilities;

/// A host capability that a selected product feature may require.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityId {
    ControllerHidAccess,
    SerialPortAccess,
    VirtualGamepadAccess,
    DesktopInputAccess,
    InputMonitoring,
    PostEvent,
    Accessibility,
}

/// The active product features from which a provider derives requirements.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each independently selectable feature contributes a distinct capability requirement"
)]
pub struct CapabilityContext {
    pub controller_input_enabled: bool,
    pub serial_output_or_firmware_enabled: bool,
    pub virtual_output_enabled: bool,
    pub desktop_bindings_enabled: bool,
    pub desktop_session: Option<DesktopSession>,
}

/// Capabilities whose ordering either is or is not significant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequirementGroup {
    /// A later capability must not even be probed until every earlier one is met.
    Ordered(Vec<CapabilityId>),
    /// Every capability may be probed independently.
    Independent(Vec<CapabilityId>),
}

/// The current state of one capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityState {
    Satisfied,
    NotRequired,
    Undecided,
    Pending,
    Blocked { reason: String },
    Unavailable { reason: String },
}

impl CapabilityState {
    #[must_use]
    pub const fn is_met(&self) -> bool {
        matches!(self, Self::Satisfied | Self::NotRequired)
    }
}

/// An interactive way to resolve an unsatisfied capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remedy {
    RequestFromSystem,
    OpenUrl(String),
    Instructions {
        text: String,
        command: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityError {
    #[error("cannot request {id:?}: {reason}")]
    Request { id: CapabilityId, reason: String },
    #[error("platform capabilities are unavailable on {platform}")]
    UnsupportedPlatform { platform: &'static str },
    #[error("no request outcome was scripted for {id:?}")]
    ScriptExhausted { id: CapabilityId },
}

/// Derives, probes, and interactively requests capabilities for one platform.
pub trait PlatformCapabilities {
    fn requirements(&self, context: &CapabilityContext) -> Vec<RequirementGroup>;

    /// Checks one capability without prompting or opening a retained session.
    fn probe(&self, id: CapabilityId) -> CapabilityState;

    /// Performs an explicitly interactive capability request.
    ///
    /// Providers retain any permit or portal session needed after this call.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the request cannot be attempted.
    fn request(&mut self, id: CapabilityId) -> Result<CapabilityState, CapabilityError>;

    fn remedy(&self, id: CapabilityId) -> Option<Remedy>;
}

/// Selects the capability provider for the current host.
///
/// # Errors
///
/// Returns [`CapabilityError::UnsupportedPlatform`] until the current target
/// has a provider.
pub fn current_provider() -> Result<Box<dyn PlatformCapabilities>, CapabilityError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(MacOsCapabilities::new()))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(LinuxCapabilities::new()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(CapabilityError::UnsupportedPlatform {
            platform: std::env::consts::OS,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRequirement {
    pub id: CapabilityId,
    pub state: CapabilityState,
    pub remedy: Option<Remedy>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CapabilityReport {
    pub unsatisfied: Vec<CapabilityRequirement>,
}

impl CapabilityReport {
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.unsatisfied.is_empty()
    }
}

/// Probes the requirements allowed by each group's ordering contract.
///
/// Ordered groups stop at their first unmet capability. Independent groups
/// probe every member so a frontend can present all applicable blockers.
#[must_use]
pub fn evaluate_requirements(
    provider: &dyn PlatformCapabilities,
    context: &CapabilityContext,
) -> CapabilityReport {
    let mut unsatisfied = Vec::new();
    for group in provider.requirements(context) {
        match group {
            RequirementGroup::Ordered(ids) => {
                for id in ids {
                    let state = provider.probe(id);
                    if state.is_met() {
                        continue;
                    }
                    unsatisfied.push(CapabilityRequirement {
                        id,
                        state,
                        remedy: provider.remedy(id),
                    });
                    break;
                }
            }
            RequirementGroup::Independent(ids) => {
                for id in ids {
                    let state = provider.probe(id);
                    if !state.is_met() {
                        unsatisfied.push(CapabilityRequirement {
                            id,
                            state,
                            remedy: provider.remedy(id),
                        });
                    }
                }
            }
        }
    }
    CapabilityReport { unsatisfied }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRequestOutcome {
    pub id: CapabilityId,
    pub previous: CapabilityState,
    pub current: CapabilityState,
    pub remedy: Option<Remedy>,
}

/// Requests the first unsatisfied capability, if any.
///
/// # Errors
///
/// Returns the provider's typed request error.
pub fn request_next(
    provider: &mut dyn PlatformCapabilities,
    context: &CapabilityContext,
) -> Result<Option<CapabilityRequestOutcome>, CapabilityError> {
    let Some(requirement) = evaluate_requirements(provider, context)
        .unsatisfied
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let current = provider.request(requirement.id)?;
    let remedy = (!current.is_met()).then_some(requirement.remedy).flatten();
    Ok(Some(CapabilityRequestOutcome {
        id: requirement.id,
        previous: requirement.state,
        current,
        remedy,
    }))
}

/// A deterministic provider for contract and consumer tests.
#[derive(Debug, Default)]
pub struct ScriptedProvider {
    requirements: Vec<RequirementGroup>,
    states: HashMap<CapabilityId, CapabilityState>,
    requests: HashMap<CapabilityId, VecDeque<Result<CapabilityState, CapabilityError>>>,
    remedies: HashMap<CapabilityId, Remedy>,
    calls: RefCell<Vec<CapabilityCall>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityCall {
    Requirements(CapabilityContext),
    Probe(CapabilityId),
    Request(CapabilityId),
    Remedy(CapabilityId),
}

impl ScriptedProvider {
    #[must_use]
    pub fn new(requirements: Vec<RequirementGroup>) -> Self {
        Self {
            requirements,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_state(mut self, id: CapabilityId, state: CapabilityState) -> Self {
        self.states.insert(id, state);
        self
    }

    #[must_use]
    pub fn with_request_state(mut self, id: CapabilityId, state: CapabilityState) -> Self {
        self.requests.entry(id).or_default().push_back(Ok(state));
        self
    }

    #[must_use]
    pub fn with_request_error(mut self, id: CapabilityId, reason: impl Into<String>) -> Self {
        self.requests
            .entry(id)
            .or_default()
            .push_back(Err(CapabilityError::Request {
                id,
                reason: reason.into(),
            }));
        self
    }

    #[must_use]
    pub fn with_remedy(mut self, id: CapabilityId, remedy: Remedy) -> Self {
        self.remedies.insert(id, remedy);
        self
    }

    pub fn set_state(&mut self, id: CapabilityId, state: CapabilityState) {
        self.states.insert(id, state);
    }

    #[must_use]
    pub fn calls(&self) -> Vec<CapabilityCall> {
        self.calls.borrow().clone()
    }

    pub fn clear_calls(&self) {
        self.calls.borrow_mut().clear();
    }
}

impl PlatformCapabilities for ScriptedProvider {
    fn requirements(&self, context: &CapabilityContext) -> Vec<RequirementGroup> {
        self.calls
            .borrow_mut()
            .push(CapabilityCall::Requirements(context.clone()));
        self.requirements.clone()
    }

    fn probe(&self, id: CapabilityId) -> CapabilityState {
        self.calls.borrow_mut().push(CapabilityCall::Probe(id));
        self.states
            .get(&id)
            .cloned()
            .unwrap_or_else(|| CapabilityState::Unavailable {
                reason: "no state was scripted".to_owned(),
            })
    }

    fn request(&mut self, id: CapabilityId) -> Result<CapabilityState, CapabilityError> {
        self.calls.get_mut().push(CapabilityCall::Request(id));
        let outcome = self
            .requests
            .get_mut(&id)
            .and_then(VecDeque::pop_front)
            .ok_or(CapabilityError::ScriptExhausted { id })?;
        if let Ok(state) = &outcome {
            self.states.insert(id, state.clone());
        }
        outcome
    }

    fn remedy(&self, id: CapabilityId) -> Option<Remedy> {
        self.calls.borrow_mut().push(CapabilityCall::Remedy(id));
        self.remedies.get(&id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(reason: &str) -> CapabilityState {
        CapabilityState::Blocked {
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn ordered_groups_never_probe_past_the_first_unmet_capability() {
        let provider = ScriptedProvider::new(vec![RequirementGroup::Ordered(vec![
            CapabilityId::InputMonitoring,
            CapabilityId::PostEvent,
            CapabilityId::Accessibility,
        ])])
        .with_state(CapabilityId::InputMonitoring, CapabilityState::Satisfied)
        .with_state(CapabilityId::PostEvent, blocked("post events are denied"))
        .with_state(
            CapabilityId::Accessibility,
            blocked("accessibility is denied"),
        );

        let report = evaluate_requirements(&provider, &CapabilityContext::default());

        assert_eq!(report.unsatisfied.len(), 1);
        assert_eq!(report.unsatisfied[0].id, CapabilityId::PostEvent);
        assert_eq!(
            provider.calls(),
            vec![
                CapabilityCall::Requirements(CapabilityContext::default()),
                CapabilityCall::Probe(CapabilityId::InputMonitoring),
                CapabilityCall::Probe(CapabilityId::PostEvent),
                CapabilityCall::Remedy(CapabilityId::PostEvent),
            ]
        );
    }

    #[test]
    fn independent_groups_report_every_unsatisfied_capability() {
        let provider = ScriptedProvider::new(vec![RequirementGroup::Independent(vec![
            CapabilityId::ControllerHidAccess,
            CapabilityId::SerialPortAccess,
        ])])
        .with_state(
            CapabilityId::ControllerHidAccess,
            blocked("hidraw is unavailable"),
        )
        .with_state(
            CapabilityId::SerialPortAccess,
            blocked("serial access is unavailable"),
        );

        let report = evaluate_requirements(&provider, &CapabilityContext::default());

        assert_eq!(
            report
                .unsatisfied
                .iter()
                .map(|requirement| requirement.id)
                .collect::<Vec<_>>(),
            [
                CapabilityId::ControllerHidAccess,
                CapabilityId::SerialPortAccess,
            ]
        );
        assert!(provider
            .calls()
            .contains(&CapabilityCall::Probe(CapabilityId::SerialPortAccess)));
    }

    #[test]
    fn request_flow_returns_pending_state_and_its_remedy() {
        let remedy = Remedy::OpenUrl("settings:input-monitoring".to_owned());
        let mut provider = ScriptedProvider::new(vec![RequirementGroup::Ordered(vec![
            CapabilityId::InputMonitoring,
        ])])
        .with_state(CapabilityId::InputMonitoring, CapabilityState::Undecided)
        .with_request_state(CapabilityId::InputMonitoring, CapabilityState::Pending)
        .with_remedy(CapabilityId::InputMonitoring, remedy.clone());

        let outcome = request_next(&mut provider, &CapabilityContext::default())
            .unwrap()
            .unwrap();

        assert_eq!(outcome.id, CapabilityId::InputMonitoring);
        assert_eq!(outcome.previous, CapabilityState::Undecided);
        assert_eq!(outcome.current, CapabilityState::Pending);
        assert_eq!(outcome.remedy, Some(remedy));
        assert_eq!(
            provider.calls(),
            vec![
                CapabilityCall::Requirements(CapabilityContext::default()),
                CapabilityCall::Probe(CapabilityId::InputMonitoring),
                CapabilityCall::Remedy(CapabilityId::InputMonitoring),
                CapabilityCall::Request(CapabilityId::InputMonitoring),
            ]
        );
    }

    #[test]
    fn a_successful_request_updates_the_scripted_probe_state() {
        let mut provider = ScriptedProvider::new(vec![RequirementGroup::Ordered(vec![
            CapabilityId::Accessibility,
        ])])
        .with_state(CapabilityId::Accessibility, blocked("not granted"))
        .with_request_state(CapabilityId::Accessibility, CapabilityState::Satisfied);

        let outcome = request_next(&mut provider, &CapabilityContext::default())
            .unwrap()
            .unwrap();

        assert_eq!(outcome.current, CapabilityState::Satisfied);
        assert_eq!(outcome.remedy, None);
        provider.clear_calls();
        assert!(evaluate_requirements(&provider, &CapabilityContext::default()).is_ready());
    }

    #[test]
    fn not_required_capabilities_do_not_block_readiness() {
        let provider = ScriptedProvider::new(vec![RequirementGroup::Independent(vec![
            CapabilityId::VirtualGamepadAccess,
        ])])
        .with_state(
            CapabilityId::VirtualGamepadAccess,
            CapabilityState::NotRequired,
        );

        assert!(evaluate_requirements(&provider, &CapabilityContext::default()).is_ready());
    }
}
