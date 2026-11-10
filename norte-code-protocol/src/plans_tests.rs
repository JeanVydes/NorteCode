use super::*;

#[test]
fn body_horizontal_rule_does_not_close_front_matter() {
    let plan = parse_plan(
        "one",
        "---\nstatus: pending\n---\n# Plan\n\n---\n\ncontinued",
    )
    .expect("valid plan");
    assert_eq!(plan.content, "# Plan\n\n---\n\ncontinued");
}

#[test]
fn metadata_prefix_is_enough_for_listing() {
    let plan = parse_plan(
        "one",
        "---\nstatus: active\nupdated_at: 2026-11-04T00:00:00Z\n---",
    )
    .expect("valid metadata");
    assert_eq!(plan.status, PlanStatus::Active);
    assert_eq!(plan.content, "");
}
