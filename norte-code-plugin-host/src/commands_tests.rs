use super::*;

/// A registry shaped like the client's: a bare command, one with a closed
/// argument, one with a template, and one taking the rest of the line.
fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(CommandSpec::new("/help", "show this list"));
    registry.register(
        CommandSpec::new("/reasoning", "set the reasoning effort")
            .arg(ArgSpec::open_with(
                "setting",
                &["off", "low", "medium", "high", "max"],
            ))
            .arg(ArgSpec::optional_choice("scope", &["session", "message"])),
    );
    registry.register(CommandSpec::new("/rename", "name the session").arg(ArgSpec::rest("title")));
    registry.register(CommandSpec::new("/refresh", "re-read everything").aliased(&["/reload"]));
    registry.register(
        CommandSpec::new("/allow", "allow a tool without asking")
            .arg(ArgSpec::required("tool"))
            .arg(ArgSpec::optional_choice("scope", &["session", "global"])),
    );
    registry
}

#[test]
fn a_command_and_its_aliases_resolve_to_the_same_thing() {
    let registry = registry();
    let typed = registry
        .parse("/refresh")
        .expect("a command")
        .expect("no arguments needed");
    assert_eq!(typed.command, "/refresh");
    let aliased = registry
        .parse("/reload")
        .expect("a command")
        .expect("the alias means the same");
    assert_eq!(
        aliased.command, "/refresh",
        "the alias reports the canonical name, so the host matches once"
    );
}

#[test]
fn text_that_is_not_a_command_is_not_one() {
    // A message that starts with a slash still belongs to the model, and
    // `/reasoningx` is a word that only looks like a command: taking the
    // longer prefix would swallow it.
    let registry = registry();
    assert!(registry.parse("hello there").is_none());
    assert!(registry.parse("/reasoningx high").is_none());
    assert!(registry.parse("/nope").is_none());
}

#[test]
fn arguments_are_matched_by_what_they_are_and_not_by_where_they_sit() {
    // The order of two typed words is not something a person reads a usage
    // line to learn: `session` is a scope wherever it appears.
    let registry = registry();
    let one = registry
        .parse("/reasoning high session")
        .expect("a command")
        .expect("both orders fit");
    let other = registry
        .parse("/reasoning session high")
        .expect("a command")
        .expect("both orders fit");
    assert_eq!(one.arg("setting"), Some("high"));
    assert_eq!(one.arg("scope"), Some("session"));
    assert_eq!(other, one, "the same invocation either way");
}

#[test]
fn a_missing_required_argument_is_an_error_naming_it() {
    let registry = registry();
    let refused = registry
        .parse("/allow")
        .expect("a command")
        .expect_err("the tool is required");
    assert!(refused.contains("tool"), "{refused}");
    assert!(
        refused.contains("Usage: /allow <tool> [session|global]"),
        "the error says what would have worked: {refused}"
    );
}

#[test]
fn a_word_no_argument_wants_is_refused_rather_than_dropped() {
    // Silently ignoring it would run the command the person did not ask
    // for, which is worse than saying it was not understood.
    let registry = registry();
    let refused = registry
        .parse("/reasoning high song")
        .expect("a command")
        .expect_err("nothing takes `song`");
    assert!(refused.contains("song"), "{refused}");
}

#[test]
fn the_rest_of_the_line_is_one_argument_spaces_and_all() {
    let registry = registry();
    let renamed = registry
        .parse("/rename fix the footer")
        .expect("a command")
        .expect("a title is not a word");
    assert_eq!(renamed.arg("title"), Some("fix the footer"));
    // And bare, it is not an error: what a bare one means is the host's.
    let bare = registry
        .parse("/rename")
        .expect("a command")
        .expect("a bare rename is a command");
    assert_eq!(bare.arg("title"), None);
}

#[test]
fn the_completion_offers_the_commands_a_prefix_names() {
    let registry = registry();
    let candidates = registry.candidates("/", 1);
    let names: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert!(names.contains(&"/reasoning"), "{names:?}");
    assert!(names.contains(&"/help"), "{names:?}");
    // An alias is offered too: a person who knows `/reload` types it.
    assert!(names.contains(&"/reload"), "{names:?}");
    assert_eq!(
        candidates.len(),
        registry
            .commands()
            .iter()
            .map(|spec| spec.spellings().len())
            .sum::<usize>(),
        "every spelling is a row"
    );
}

#[test]
fn the_completion_narrows_to_what_was_typed_and_says_what_to_add() {
    let registry = registry();
    let candidates = registry.candidates("/rea", 4);
    assert_eq!(
        candidates.len(),
        1,
        "only `/reasoning` starts with `/rea`: {candidates:?}"
    );
    let reasoning = candidates
        .iter()
        .find(|candidate| candidate.text == "/reasoning")
        .expect("the one that matches");
    assert_eq!(
        reasoning.completion, "soning",
        "Tab appends the rest, so the line keeps what was typed"
    );
}

#[test]
fn a_closed_argument_completes_from_its_own_values() {
    let registry = registry();
    let candidates = registry.candidates("/reasoning ", "/reasoning ".len());
    let values: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert!(values.contains(&"high"), "{values:?}");
    assert!(values.contains(&"medium"), "{values:?}");

    // A prefix narrows it, which is the case that has to work: `xh` is
    // `xhigh` and nothing else.
    let mut registry = Registry::new();
    registry.register(
        CommandSpec::new("/reasoning", "set the effort").arg(ArgSpec::choice(
            "setting",
            &[
                "auto", "off", "minimal", "low", "medium", "high", "xhigh", "max",
            ],
        )),
    );
    let candidates = registry.candidates("/reasoning xh", "/reasoning xh".len());
    assert_eq!(candidates.len(), 1, "{candidates:?}");
    assert_eq!(candidates[0].text, "xhigh");
    assert_eq!(candidates[0].completion, "igh");
}

#[test]
fn an_argument_already_given_is_not_offered_again() {
    // `/reasoning high ` has one word left to say, and it is the scope.
    let registry = registry();
    let candidates = registry.candidates("/reasoning high ", "/reasoning high ".len());
    let values: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert_eq!(values, ["session", "message"], "{candidates:?}");
}

#[test]
fn a_template_value_completes_to_its_prefix_and_not_past_it() {
    // `budget:<tokens>` is typed as `budget:` and a number; completing the
    // whole template would put `<tokens>` on the line.
    let mut registry = Registry::new();
    registry.register(
        CommandSpec::new("/reasoning", "set the effort")
            .arg(ArgSpec::choice("setting", &["high", "budget:<tokens>"])),
    );
    let candidates = registry.candidates("/reasoning bud", "/reasoning bud".len());
    assert_eq!(candidates.len(), 1, "{candidates:?}");
    assert_eq!(candidates[0].text, "budget:<tokens>");
    assert_eq!(candidates[0].completion, "get:");

    // And the value itself parses with its number.
    let parsed = registry
        .parse("/reasoning budget:4096")
        .expect("a command")
        .expect("a budget is the value with its number");
    assert_eq!(parsed.arg("setting"), Some("budget:4096"));
}

#[test]
fn the_rest_of_the_line_keeps_its_own_spacing() {
    // A title with two spaces in the middle is a different title from one
    // with a single space, and reassembling the words would lose that.
    let registry = registry();
    let renamed = registry
        .parse("/rename fix  the footer")
        .expect("a command")
        .expect("a title");
    assert_eq!(
        renamed.arg("title"),
        Some("fix  the footer"),
        "the rest is taken literally"
    );
    // Leading and trailing space is not part of it: the line was trimmed.
    let padded = registry
        .parse("/rename   padded   ")
        .expect("a command")
        .expect("a title");
    assert_eq!(padded.arg("title"), Some("padded"));
}

#[test]
fn the_usage_line_is_written_from_the_declaration() {
    // One place writes it, so the help, the completion and the error all
    // say the same thing.
    let registry = registry();
    let allow = registry.get("/allow").expect("declared");
    assert_eq!(allow.usage(), "/allow <tool> [session|global]");
    let rename = registry.get("/rename").expect("declared");
    assert_eq!(rename.usage(), "/rename [title]");
    let help = registry.get("/help").expect("declared");
    assert_eq!(help.usage(), "/help");
}

#[test]
fn registering_a_name_again_replaces_the_command() {
    // A plugin that wants a command to do something else declares it, and
    // the last declaration is the one that is true.
    let mut registry = registry();
    let before = registry.commands().len();
    registry.register(CommandSpec::new("/help", "say something else"));
    assert_eq!(registry.commands().len(), before, "one command, not two");
    assert_eq!(
        registry.get("/help").expect("declared").summary,
        "say something else"
    );
}

#[test]
fn an_alias_and_a_name_do_not_both_survive_a_replace() {
    // `/reload` is an alias of `/refresh`; a command that takes the name
    // `/reload` for itself replaces the one that answered to it.
    let mut registry = registry();
    registry.register(CommandSpec::new("/reload", "something else entirely"));
    let typed = registry
        .parse("/reload")
        .expect("a command")
        .expect("no args");
    assert_eq!(
        typed.command, "/reload",
        "the name now means the command that took it"
    );
    assert!(
        registry.get("/refresh").is_some(),
        "the command that used to own the alias is still there"
    );
    assert!(
        !registry
            .get("/refresh")
            .expect("declared")
            .aliases
            .contains(&"/reload".to_owned()),
        "but it no longer answers to the name that was taken"
    );
}
