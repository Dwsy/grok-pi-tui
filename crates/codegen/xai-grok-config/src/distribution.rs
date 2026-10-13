//! What a build of grok may do. Call sites ask [`Distribution::allows`] instead of testing Cargo
//! features, so each distribution's answers are testable in any build.
//!
//! A withheld capability stays off whatever config, requirements or the environment say. Keeping a
//! service's code out of the binary is the Cargo feature set's job.
/// The capabilities a build of grok withholds. [`Distribution::current`] is this build's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Distribution {
    withheld: u32,
}
/// Something a distribution may withhold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::EnumIter)]
pub enum Capability {
    /// Product telemetry and trace upload.
    Telemetry,
    /// Crash and error reports.
    ErrorReporting,
    /// `/v1/models` and `/v1/settings` fetches and the managed-config sync.
    RemoteFetch,
    /// A login kept in `auth.json` or `GROK_AUTH`, an auth provider command, and signing in to or
    /// out of an account. Without it the only credential is the API key the build was given.
    AccountLogin,
    /// Controlling this machine from other devices.
    RemoteControl,
    /// Voice dictation, whose speech goes to a speech-to-text service.
    Voice,
}
impl Capability {
    const fn bit(self) -> u32 {
        1 << self as u32
    }
}
impl Distribution {
    /// Every capability, subject to config, requirements and the environment.
    pub const STOCK: Self = Self { withheld: 0 };
    /// A distribution that withholds `capabilities` and allows the rest.
    pub const fn withholding(capabilities: &[Capability]) -> Self {
        let mut withheld = 0;
        let mut rest = capabilities;
        while let [capability, tail @ ..] = rest {
            withheld |= capability.bit();
            rest = tail;
        }
        Self { withheld }
    }
    /// This build's distribution.
    pub const fn current() -> Self {
        Self::STOCK
    }
    pub const fn allows(self, capability: Capability) -> bool {
        self.withheld & capability.bit() == 0
    }
    /// What a surface that offers `capability` tells the user when this distribution withholds it.
    /// `None` where the distribution allows it, or where no surface offers it to refuse.
    pub const fn refusal(self, capability: Capability) -> Option<&'static str> {
        if self.allows(capability) {
            return None;
        }
        match capability {
            Capability::AccountLogin => Some(
                "This build has no account to sign in to or out of: it uses only the API key it was \
                 given.",
            ),
            Capability::RemoteControl => Some("Remote Control is off in this build."),
            Capability::Voice => Some("Voice dictation is off in this build."),
            Capability::Telemetry | Capability::ErrorReporting | Capability::RemoteFetch => None,
        }
    }
}
#[cfg(test)]
#[path = "distribution_tests.rs"]
mod tests;
