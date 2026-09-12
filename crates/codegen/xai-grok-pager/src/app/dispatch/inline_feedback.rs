//! `/feedback <text>`: the drain-time predraft write; the drain and the modal send share [`select_feedback_images`], so a dropped image never drops the report.

use std::path::Path;

use crate::app::agent_view::{AgentView, PromptInputMode, PromptMode};
use xai_grok_feedback::{
    DraftImage, DraftImagePolicy, FeedbackDraftId, FeedbackDraftStore, FeedbackStoreError,
    derive_title, read_draft_images, write_draft_images,
};
use xai_grok_shell::session::{
    MAX_FEEDBACK_IMAGE_BYTES, MAX_FEEDBACK_IMAGE_TOTAL_BYTES, MAX_FEEDBACK_IMAGES,
    feedback_image_extension,
};

/// Pushed once per requeued row: a bare Enter on an empty composer drains nothing, so the gesture is a real send.
pub(super) const FEEDBACK_STORE_BUSY_NOTICE: &str = "Could not save the feedback draft right now (another process has it open); the report stays queued. Send your next message to retry.";

/// Applies the send-time image policy in order; `None` is an image whose bytes could not be read or decoded.
/// Returns the accepted indices plus the user notice for anything dropped.
pub(crate) fn select_feedback_images(
    images: &[Option<(Vec<u8>, String)>],
) -> (Vec<usize>, Option<String>) {
    let mut accepted = Vec::new();
    let mut over_count = 0usize;
    let mut unsupported = 0usize;
    let mut too_large = 0usize;
    let mut unreadable = 0usize;
    let mut total_bytes = 0usize;
    for (index, image) in images.iter().enumerate() {
        let Some((bytes, mime_type)) = image.as_ref().filter(|(bytes, _)| !bytes.is_empty()) else {
            unreadable += 1;
            continue;
        };
        if accepted.len() >= MAX_FEEDBACK_IMAGES {
            over_count += 1;
            continue;
        }
        if feedback_image_extension(mime_type).is_none() {
            unsupported += 1;
            continue;
        }
        if bytes.len() > MAX_FEEDBACK_IMAGE_BYTES
            || total_bytes + bytes.len() > MAX_FEEDBACK_IMAGE_TOTAL_BYTES
        {
            too_large += 1;
            continue;
        }
        total_bytes += bytes.len();
        accepted.push(index);
    }
    let dropped = over_count + unsupported + too_large + unreadable;
    let notice = (dropped > 0).then(|| {
        const MIB: usize = 1024 * 1024;
        let mut reasons = Vec::new();
        if over_count > 0 {
            reasons.push(format!(
                "{over_count} over the {MAX_FEEDBACK_IMAGES}-image limit"
            ));
        }
        if unsupported > 0 {
            reasons.push(format!(
                "{unsupported} in a format feedback can't carry (PNG, JPEG, or GIF only)"
            ));
        }
        if too_large > 0 {
            reasons.push(format!(
                "{too_large} over the size limit ({} MB each, {} MB combined)",
                MAX_FEEDBACK_IMAGE_BYTES / MIB,
                MAX_FEEDBACK_IMAGE_TOTAL_BYTES / MIB,
            ));
        }
        if unreadable > 0 {
            reasons.push(format!("{unreadable} unreadable"));
        }
        let plural = if dropped == 1 { "" } else { "s" };
        format!(
            "Dropped {dropped} image{plural} from the feedback: {}.",
            reasons.join(", ")
        )
    });
    (accepted, notice)
}

#[derive(Debug)]
pub(super) struct InlineDraftSaved {
    pub(super) draft_id: FeedbackDraftId,
    /// Images dropped by policy or lost to a write failure; the text is saved either way.
    pub(super) notice: Option<String>,
}

#[derive(Debug)]
pub(super) enum InlineDraftSaveError {
    /// Another process holds the store lock; the row can be retried unchanged.
    Busy,
    Failed(String),
}

const DRAFT_IMAGE_POLICY: DraftImagePolicy = DraftImagePolicy {
    max_images: MAX_FEEDBACK_IMAGES,
    max_image_bytes: MAX_FEEDBACK_IMAGE_BYTES,
    extension_for_mime: feedback_image_extension,
};

/// `images` are the row's wire images decoded to `(bytes, mime_type)`, `None` where decoding failed.
/// Images are written after the JSON commit so they are keyed by a real id; a write failure downgrades to a notice instead of deleting the draft.
pub(super) fn save_inline_feedback_draft(
    session_dir: Option<&Path>,
    user_text: &str,
    images: &[Option<(Vec<u8>, String)>],
) -> Result<InlineDraftSaved, InlineDraftSaveError> {
    let Some(session_dir) = session_dir else {
        return Err(InlineDraftSaveError::Failed("No active session".to_owned()));
    };
    let (accepted, mut notice) = select_feedback_images(images);
    let draft = FeedbackDraftStore::new(session_dir)
        .append_predraft(&derive_title(user_text), user_text)
        .map_err(|error| match error {
            FeedbackStoreError::Busy => InlineDraftSaveError::Busy,
            FeedbackStoreError::InvalidSessionDirectory { .. } => {
                InlineDraftSaveError::Failed("No active session".to_owned())
            }
            other => InlineDraftSaveError::Failed(format!(
                "Could not save the local feedback draft: {other}"
            )),
        })?;
    let accepted: Vec<DraftImage> = accepted
        .iter()
        .filter_map(|&index| images[index].as_ref())
        .map(|(bytes, mime_type)| DraftImage {
            bytes: bytes.clone(),
            mime_type: mime_type.clone(),
        })
        .collect();
    if let Err(error) = write_draft_images(session_dir, &draft.id, &accepted, DRAFT_IMAGE_POLICY) {
        let error = format!("Could not save feedback draft images: {error}");
        notice = Some(match notice {
            Some(notice) => format!("{notice} {error}"),
            None => error,
        });
    }
    Ok(InlineDraftSaved {
        draft_id: draft.id,
        notice,
    })
}

/// Puts a report whose draft could not be saved back where the user can recover it and returns the notice to show.
/// The composer is spliced through [`crate::views::prompt_widget::PromptWidget::prepend_text`] (never `set_text`, which drops its chips). While it holds a queued row's edit buffer or a non-prompt input mode (`!`/`#`) the composer is left alone and the report is echoed in the notice instead.
pub(super) fn restore_inline_feedback_report(
    agent: &mut AgentView,
    report: &str,
    images: Vec<(Vec<u8>, String)>,
    failure: String,
) -> String {
    let composer_is_taken = matches!(agent.prompt_mode, PromptMode::EditingQueued { .. })
        || agent.prompt_input_mode != PromptInputMode::Normal;
    if composer_is_taken {
        let images_note = match images.len() {
            0 => String::new(),
            1 => " (its image was dropped)".to_owned(),
            count => format!(" (its {count} images were dropped)"),
        };
        return format!("{failure}\nNot sent: {report}{images_note}");
    }
    let separator = if agent.prompt.text().is_empty() {
        ""
    } else {
        "\n"
    };
    // The chips go after the report on its own line, so leave room for them before the separator.
    let chip_gap = if images.is_empty() { "" } else { " " };
    agent
        .prompt
        .prepend_text(&format!("{report}{chip_gap}{separator}"));
    agent.prompt.set_cursor(report.len() + chip_gap.len());
    let mut refused = 0usize;
    for (data, mime_type) in images {
        let image = crate::prompt_images::from_clipboard_data(&crate::clipboard::ImageData {
            data,
            mime_type,
        });
        if agent.prompt.insert_image(image).is_err() {
            refused += 1;
        }
    }
    if refused == 0 {
        return failure;
    }
    let plural = if refused == 1 { "" } else { "s" };
    format!("{failure}; {refused} image{plural} could not be restored.")
}

pub(super) fn attach_saved_draft_images(
    modal: &mut crate::views::feedback_modal::FeedbackModalState,
    session_dir: &Path,
    draft_id: &FeedbackDraftId,
) {
    for image in read_feedback_draft_images(session_dir, draft_id) {
        let _ = modal.insert_image(image);
    }
}

pub(super) fn read_feedback_draft_images(
    session_dir: &Path,
    draft_id: &FeedbackDraftId,
) -> Vec<crate::prompt_images::PastedImage> {
    read_draft_images(session_dir, draft_id, DRAFT_IMAGE_POLICY)
        .into_iter()
        .map(|image| {
            crate::prompt_images::from_clipboard_data(&crate::clipboard::ImageData {
                data: image.bytes,
                mime_type: image.mime_type,
            })
        })
        .collect()
}
