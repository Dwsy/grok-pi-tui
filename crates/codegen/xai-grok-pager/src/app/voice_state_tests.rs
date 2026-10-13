use xai_grok_voice::VoiceRoute;

use super::{Partial, VoiceState, VoiceTarget};
use crate::app::agent::AgentId;

fn target() -> VoiceTarget {
    VoiceTarget::Agent(AgentId(0))
}

fn recording(route: Option<VoiceRoute>, partial: Partial) -> VoiceState {
    VoiceState::Recording {
        hold: false,
        target: target(),
        partial,
        route,
    }
}

fn stopping(route: Option<VoiceRoute>, partial: Partial) -> VoiceState {
    VoiceState::Stopping {
        target: target(),
        partial,
        route,
    }
}

fn transcribing(partial: Partial) -> VoiceState {
    VoiceState::Transcribing {
        target: target(),
        partial,
    }
}

fn shown() -> Partial {
    Partial::Shown("words".to_owned())
}

/// Per state: whether the session still owes its final (only a clip that has shown no partial; streaming commits
/// on the way, an unreported route has captured nothing worth waiting for) and whether a press must wait (only
/// while a clip is outstanding; a stop the pipeline never classified, a quick tap, must not wedge the next press).
#[test]
fn owes_final_and_blocks_new_capture_by_state() {
    let cold = VoiceState::ColdStart {
        hold: false,
        target: target(),
    };
    for (state, owes, blocks) in [
        (VoiceState::Idle, false, false),
        (cold, false, false),
        (
            recording(Some(VoiceRoute::Streaming), Partial::None),
            false,
            false,
        ),
        (
            stopping(Some(VoiceRoute::Streaming), Partial::None),
            false,
            false,
        ),
        (
            recording(Some(VoiceRoute::Clip), Partial::None),
            true,
            false,
        ),
        (recording(Some(VoiceRoute::Clip), shown()), false, false),
        (
            recording(Some(VoiceRoute::Clip), Partial::Committed),
            false,
            false,
        ),
        (stopping(Some(VoiceRoute::Clip), Partial::None), true, true),
        (stopping(Some(VoiceRoute::Clip), shown()), false, true),
        (recording(None, Partial::None), false, false),
        (stopping(None, Partial::None), false, false),
        (transcribing(Partial::None), true, true),
        (transcribing(shown()), false, true),
        (transcribing(Partial::Committed), false, true),
    ] {
        assert_eq!(
            (owes, blocks),
            (state.owes_final(), state.blocks_new_capture()),
            "{state:?}"
        );
    }
}

/// Only a shown partial is on the overlay; a committed one has left it but still counts as shown for `owes_final`.
#[test]
fn interim_is_only_the_shown_partial() {
    assert_eq!(
        Some("words"),
        recording(Some(VoiceRoute::Clip), shown()).interim()
    );
    assert_eq!(
        None,
        recording(Some(VoiceRoute::Clip), Partial::Committed).interim()
    );
    assert_eq!(None, transcribing(Partial::None).interim());
    assert_eq!(None, VoiceState::Idle.interim());
}
