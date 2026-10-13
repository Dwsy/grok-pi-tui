//! Agent-view tests for the model notice banner above the prompt.
use super::{AgentView, AppRenderParams, BannerSlotParams, test_fixtures};
use crate::actions::ActionRegistry;
use crate::scrollback::render::ScratchBuffer;
use agent_client_protocol as acp;
use pretty_assertions::assert_eq;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
const RESTRICTED_ID: &str = "example-model-restricted";
const PLAIN_ID: &str = "example-model";
const RESTRICTED_TEXT: &str = "Reminder: this model has usage restrictions. See your admin for details before you share any code or data with it in this session.";
const TIP: &str = "Use /model to switch models";
fn model(
    id: &str,
    name: &str,
    notice: Option<serde_json::Value>,
) -> (acp::ModelId, acp::ModelInfo) {
    let model_id = acp::ModelId::new(id);
    let mut meta = serde_json::Map::new();
    if let Some(notice) = notice {
        meta.insert("notice".to_owned(), notice);
    }
    let info = acp::ModelInfo::new(model_id.clone(), name.to_owned()).meta(meta);
    (model_id, info)
}
/// An agent whose catalog has a model with a warning notice and one without.
fn agent_on(current: &str) -> AgentView {
    let mut agent = test_fixtures::make_agent();
    agent.session.models.available = [
        model(
            RESTRICTED_ID,
            "Example Model Restricted",
            Some(serde_json::json!({"severity": "warning", "text": RESTRICTED_TEXT})),
        ),
        model(PLAIN_ID, "Example Model", None),
    ]
    .into_iter()
    .collect();
    agent
        .session
        .models
        .set_current(acp::ModelId::new(current), None);
    agent
}
fn draw(
    agent: &mut AgentView,
    width: u16,
    height: u16,
    tip: Option<&str>,
    listening: bool,
) -> Vec<String> {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    let banner = BannerSlotParams {
        height: u16::from(tip.is_some()),
        tip,
        ..BannerSlotParams::none()
    };
    agent.draw(
        area,
        &mut buf,
        &ActionRegistry::defaults(),
        &mut ScratchBuffer::new(),
        None,
        false,
        banner,
        &crate::app::bundle::BundleState::default(),
        false,
        &mut Vec::new(),
        AppRenderParams {
            voice_available: listening,
            voice_listening: listening,
            ..Default::default()
        },
    );
    (0..height)
        .map(|y| {
            (0..width)
                .filter_map(|x| buf.cell((x, y)).map(|cell| cell.symbol().to_owned()))
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .collect()
}
/// The notice's words in paint order, or `None` when no row starts with the alert prefix.
fn painted_notice(rows: &[String]) -> Option<String> {
    let start = rows.iter().position(|row| row.starts_with("! Reminder:"))?;
    let text = rows
        .iter()
        .skip(start)
        .take(crate::views::model_notice_banner::MAX_ROWS.into())
        .map(|row| row.trim_start_matches("! ").to_owned())
        .take_while(|row| RESTRICTED_TEXT.contains(row.as_str()) && !row.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Some(text)
}
fn row_of(rows: &[String], needle: &str) -> Option<usize> {
    rows.iter().position(|row| row.contains(needle))
}
#[test]
fn selected_model_notice_shows_in_full_at_common_widths() {
    for (width, height) in [(80, 24), (100, 40), (120, 40)] {
        let mut agent = agent_on(RESTRICTED_ID);
        let rows = draw(&mut agent, width, height, None, false);
        assert_eq!(
            Some(RESTRICTED_TEXT.to_owned()),
            painted_notice(&rows),
            "{width}x{height}:\n{}",
            rows.join("\n")
        );
    }
}
#[test]
fn notice_follows_the_current_model() {
    let mut agent = agent_on(RESTRICTED_ID);
    assert!(painted_notice(&draw(&mut agent, 100, 40, None, false)).is_some());
    agent
        .session
        .models
        .set_current(acp::ModelId::new(PLAIN_ID), None);
    let rows = draw(&mut agent, 100, 40, None, false);
    assert_eq!(None, painted_notice(&rows), "{}", rows.join("\n"));
    agent
        .session
        .models
        .set_current(acp::ModelId::new(RESTRICTED_ID), None);
    assert!(painted_notice(&draw(&mut agent, 100, 40, None, false)).is_some());
}
#[test]
fn notice_keeps_the_tip_and_record_rows_visible() {
    for (width, height) in [(80, 24), (100, 40)] {
        let mut agent = agent_on(RESTRICTED_ID);
        let rows = draw(&mut agent, width, height, Some(TIP), true);
        let dump = rows.join("\n");
        let tip = row_of(&rows, TIP).unwrap_or_else(|| panic!("tip row is visible:\n{dump}"));
        let notice =
            row_of(&rows, "! Reminder:").unwrap_or_else(|| panic!("notice is visible:\n{dump}"));
        let recording =
            row_of(&rows, "Recording").unwrap_or_else(|| panic!("record row is visible:\n{dump}"));
        assert!(
            tip < notice && notice < recording,
            "tip, notice, record row in order:\n{dump}"
        );
        assert_eq!(
            Some(RESTRICTED_TEXT.to_owned()),
            painted_notice(&rows),
            "{dump}"
        );
    }
}
