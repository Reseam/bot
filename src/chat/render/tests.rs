use std::time::Instant;

use super::*;

fn renderer() -> Renderer {
    Renderer {
        messages: Vec::new(),
        answer: String::new(),
        running: Vec::new(),
        finished: 0,
        failed: 0,
        rendered: Vec::new(),
        last_edit: Instant::now(),
        separate_next_text: false,
        turn_start: 0,
        compacted: false,
    }
}

fn answer_after(events: Vec<AgentEvent>) -> String {
    let mut renderer = renderer();
    events.into_iter().for_each(|event| renderer.apply(event));
    renderer.answer
}

fn text(value: &str) -> AgentEvent {
    AgentEvent::Text(value.to_owned())
}

#[test]
fn restarted_turn_discards_only_its_own_text() {
    let answer = answer_after(vec![
        AgentEvent::TurnStarted,
        text("first "),
        AgentEvent::TurnStarted,
        text("partial"),
        AgentEvent::TurnRestarted,
        text("second"),
    ]);

    assert_eq!(answer, "first\n\nsecond");
}

#[test]
fn restart_before_any_text_keeps_earlier_turns() {
    let answer = answer_after(vec![
        AgentEvent::TurnStarted,
        text("first"),
        AgentEvent::TurnStarted,
        AgentEvent::TurnRestarted,
        text("second"),
    ]);

    assert_eq!(answer, "first\n\nsecond");
}
