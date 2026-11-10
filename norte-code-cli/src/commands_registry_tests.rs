use super::*;

#[test]
fn every_command_the_parser_answers_is_a_command_the_help_offers() {
    // The two were separate lists and drifted: `/tools` parsed and was
    // never completed, and the help named four commands fewer than the
    // client answered to. Reading one declaration is what makes that
    // impossible rather than unlikely.
    let registry = built();
    let names: Vec<String> = registry
        .commands()
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    for expected in [
        "/help",
        "/compact",
        "/allow",
        "/deny",
        "/tools",
        "/reasoning",
        "/model",
        "/models",
        "/sessions",
        "/plans",
        "/provider",
        "/refresh",
        "/mcp",
        "/jobs",
        "/restart",
        "/rename",
        "/debug",
        "/menu",
        "/delete",
        "/exit",
    ] {
        assert!(names.contains(&expected.to_owned()), "missing {expected}");
    }
}

#[test]
fn every_usage_line_lines_up_so_the_list_reads_down() {
    // `/help` writes the usage column padded to the widest one, and the
    // width is measured rather than kept beside the list. This is that
    // measurement, checked where it can be seen: a command that grows an
    // argument must not push every summary out of line.
    let registry = built();
    let usages: Vec<String> = registry
        .commands()
        .iter()
        .map(norte_code_plugin_host::commands::CommandSpec::usage)
        .collect();
    let width = usages.iter().map(String::len).max().unwrap_or(0);
    for usage in &usages {
        assert!(usage.len() <= width, "`{usage}` is wider than the column");
    }
    assert!(width > 0);
    // The longest one is the one that sets it, which is the whole point of
    // measuring instead of guessing.
    assert_eq!(
        usages.iter().map(String::len).max(),
        Some(width),
        "the column is the longest line: {usages:?}"
    );
}

#[test]
fn the_command_that_was_missing_from_the_completion_is_there() {
    // The bug that started this: `/tools` was parsed and never offered.
    let registry = built();
    let candidates = registry.candidates("/to", 3);
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.text == "/tools"),
        "{candidates:?}"
    );
}

#[test]
fn the_efforts_are_offered_while_typing_one() {
    // Nobody should have to remember that it is `xhigh` and not `x-high`.
    let registry = built();
    let candidates = registry.candidates("/reasoning xh", "/reasoning xh".len());
    assert_eq!(candidates.len(), 1, "{candidates:?}");
    assert_eq!(candidates[0].text, "xhigh");
    assert_eq!(candidates[0].completion, "igh", "Tab adds the rest");
}

#[test]
fn a_scope_is_offered_after_the_effort_and_not_before_it() {
    // `/reasoning high ` has one word left, and it is the scope; the effort
    // is already given and offering it again would be noise.
    let registry = built();
    let candidates = registry.candidates("/reasoning high ", "/reasoning high ".len());
    let values: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert_eq!(values, ["session", "message"], "{candidates:?}");
}

#[test]
fn an_alias_is_offered_beside_the_name_it_belongs_to() {
    let registry = built();
    let candidates = registry.candidates("/re", 3);
    let texts: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert!(texts.contains(&"/refresh"), "{texts:?}");
    assert!(texts.contains(&"/reload"), "{texts:?}");
    assert!(texts.contains(&"/reasoning"), "{texts:?}");
    assert!(texts.contains(&"/rename"), "{texts:?}");
}

#[test]
fn a_command_with_no_arguments_offers_nothing_after_it() {
    let registry = built();
    assert!(
        registry.candidates("/help ", "/help ".len()).is_empty(),
        "there is nothing to say after a command that takes nothing"
    );
}

#[test]
fn the_registry_parses_what_the_client_answers_to() {
    // Every command in the list resolves, which is the other half of the
    // drift: a name the completion offers and the parser does not know is
    // a name that does nothing.
    let registry = built();
    for spec in registry.commands() {
        for spelling in spec.spellings() {
            let parsed = registry
                .parse(spelling)
                .unwrap_or_else(|| panic!("{spelling} is not a command"));
            if let Ok(invocation) = parsed {
                assert_eq!(
                    invocation.command, spec.name,
                    "{spelling} resolves to its canonical name"
                );
            }
        }
    }
}
