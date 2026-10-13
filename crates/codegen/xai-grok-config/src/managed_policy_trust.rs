use std::path::Path;

use crate::managed_cache::ManagedPolicyCompromise;
use crate::managed_cache::ServingIdentity;
use crate::managed_cache::managed_policy_compromised_for_at;
use crate::validation::RequirementsLayer;

/// `Unmanaged` means no requirements layers.
/// `Trusted` means layers are present and there is no tamper evidence, including when the home does not resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedPolicyTrust {
    Unmanaged,
    Trusted,
    Compromised(ManagedPolicyCompromise),
}

impl ManagedPolicyTrust {
    /// Agrees with [`crate::managed_policy_compromised_for`] on every input.
    #[must_use]
    pub fn evaluate(
        home: Option<&Path>,
        layers: &[RequirementsLayer],
        identity: &ServingIdentity,
    ) -> ManagedPolicyTrust {
        let compromise = home.and_then(|home| managed_policy_compromised_for_at(home, identity));
        match compromise {
            Some(reason) => ManagedPolicyTrust::Compromised(reason),
            None if layers.is_empty() => ManagedPolicyTrust::Unmanaged,
            None => ManagedPolicyTrust::Trusted,
        }
    }
}

#[cfg(test)]
#[path = "managed_policy_trust_tests.rs"]
mod tests;
