//! The interactive command registry.
//!
//! A slash command used to be five facts in five places: a name in a list for
//! the completion, an arm in the parser, an entry in a usage table, another in
//! the argument table, and a line in the help. Adding one meant finding all
//! five, and the day one was missed the command was half there — `/tools`
//! parsed and never completed. This is that fifth place made one: a command is
//! declared once, and the parse, the completion, the usage line and the help
//! are all read from the declaration.
//!
//! Nothing here knows what a command does. An [`Invocation`] names the command
//! and carries what it was given, and the host matches on the name; that is what
//! makes the registry usable by something that is not this binary, because a
//! registry a plugin can write into cannot be keyed by a host's own enum.
//!
//! # Shape
//!
//! A [`CommandSpec`] is a name, its aliases, a one-line summary and a list of
//! [`ArgSpec`]. An argument is closed (a set of values), open (a word the host
//! will read, like a tool name) or the rest of the line (a title, spaces and
//! all).
//!
//! # Parsing
//!
//! Arguments are matched by what a token *is*, not only by where it sits: a
//! closed value fills its argument wherever it appears. That is what lets
//! `/reasoning high session` and `/reasoning session high` both mean the same
//! thing without the parser saying so twice.
//!
//! [`Registry::parse`] returns `None` for text that is not a command, so an
//! ordinary message that happens to start with `/` still reaches the model; and
//! `Some(Err(_))` for a command whose arguments do not fit, which the caller
//! must report rather than send.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// What an argument accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Values {
    /// A closed set. The completion offers these and nothing else.
    ///
    /// A value may be a template — `budget:<tokens>` — in which case a token is
    /// taken when it starts with everything before the opening `<`, and the
    /// completion stops at the `:` so the number can be typed after it.
    Closed(Vec<String>),
    /// Any word. A tool name, a model reference: the host reads it and says
    /// what is wrong with it, because only the host knows.
    Open,
    /// A closed set the completion offers, or failing that any word.
    ///
    /// `/reasoning` is the case: the effort is one the model in play declares,
    /// so the parser cannot enforce a fixed set — but the ones that exist are
    /// what a person wants to see while typing, and they are not something to
    /// keep in their head.
    OpenWith(Vec<String>),
    /// Everything left on the line, spaces included.
    ///
    /// It is always the last argument: a title is not a word, and splitting one
    /// would make `/rename fix the footer` a usage error instead of the name it
    /// obviously is.
    Rest,
}

/// One argument of a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgSpec {
    /// The name the usage line and the errors use.
    pub name: String,
    /// What it accepts.
    pub values: Values,
    /// Whether the command is a usage error without it.
    pub required: bool,
}

impl ArgSpec {
    /// A required argument taking any word.
    #[must_use]
    pub fn required(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            values: Values::Open,
            required: true,
        }
    }

    /// A required argument taking one of a closed set.
    #[must_use]
    pub fn choice(name: &str, values: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            values: Values::Closed(values_to_owned(values)),
            required: true,
        }
    }

    /// A required argument the completion suggests values for, but that takes
    /// any word.
    ///
    /// `/reasoning` is the case: the effort is one the model declares, so the
    /// parser cannot refuse a name — the model in play decides that — but the
    /// ones that exist are what a person wants to see while typing.
    #[must_use]
    pub fn open_with(name: &str, values: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            values: Values::OpenWith(values_to_owned(values)),
            required: true,
        }
    }

    /// An optional argument the completion suggests values for, but that takes
    /// any word.
    ///
    /// `/tab` is the case: a tab is named or numbered, and the names are what a
    /// person wants to see while `2` is a perfectly good answer.
    #[must_use]
    pub fn optional_open_with(name: &str, values: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            values: Values::OpenWith(values_to_owned(values)),
            required: false,
        }
    }

    /// An optional argument taking any word.
    #[must_use]
    pub fn optional(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            values: Values::Open,
            required: false,
        }
    }

    /// An optional argument taking one of a closed set.
    #[must_use]
    pub fn optional_choice(name: &str, values: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            values: Values::Closed(values_to_owned(values)),
            required: false,
        }
    }

    /// The rest of the line, which is never required: a bare `/rename` means
    /// what it means, and only the host knows what that is.
    #[must_use]
    pub fn rest(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            values: Values::Rest,
            required: false,
        }
    }

    /// How the usage line writes it: `<name>` when it is required, `[name]`
    /// when it is not, and the closed values listed inside.
    #[must_use]
    pub fn usage(&self) -> String {
        let inner = match &self.values {
            Values::Closed(values) | Values::OpenWith(values) => {
                let listed = values
                    .iter()
                    .map(|value| match value.split_once('<') {
                        Some((prefix, _)) => prefix.to_owned(),
                        None => value.clone(),
                    })
                    .collect::<Vec<String>>()
                    .join("|");
                if listed.is_empty() {
                    self.name.clone()
                } else {
                    listed
                }
            }
            Values::Open | Values::Rest => self.name.clone(),
        };
        if self.required {
            format!("<{inner}>")
        } else {
            format!("[{inner}]")
        }
    }
}

fn values_to_owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// One command, as the registry knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    /// The canonical name, with its slash.
    pub name: String,
    /// Other spellings that mean the same command.
    pub aliases: Vec<String>,
    /// One line for the help and the completion.
    pub summary: String,
    /// What it takes, in the order the usage line reads.
    pub args: Vec<ArgSpec>,
}

impl CommandSpec {
    /// A command with no arguments.
    #[must_use]
    pub fn new(name: &str, summary: &str) -> Self {
        Self {
            name: name.to_owned(),
            aliases: Vec::new(),
            summary: summary.to_owned(),
            args: Vec::new(),
        }
    }

    /// The same command, also reachable by these names.
    #[must_use]
    pub fn aliased(mut self, aliases: &[&str]) -> Self {
        self.aliases = aliases.iter().map(|name| (*name).to_owned()).collect();
        self
    }

    /// The same command with an argument.
    #[must_use]
    pub fn arg(mut self, arg: ArgSpec) -> Self {
        self.args.push(arg);
        self
    }

    /// Every spelling of this command, the canonical name first.
    #[must_use]
    pub fn spellings(&self) -> Vec<&str> {
        std::iter::once(self.name.as_str())
            .chain(self.aliases.iter().map(String::as_str))
            .collect()
    }

    /// The usage line: the name and its arguments, which is also what the help
    /// prints beside it.
    #[must_use]
    pub fn usage(&self) -> String {
        let mut line = self.name.clone();
        for arg in &self.args {
            line.push(' ');
            line.push_str(&arg.usage());
        }
        line
    }
}

/// One command the person typed, with what they gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    /// The canonical name, even when an alias was typed.
    pub command: String,
    /// The arguments that were given, by the name the spec gave them.
    pub args: BTreeMap<String, String>,
}

impl Invocation {
    /// The value of one argument.
    #[must_use]
    pub fn arg(&self, name: &str) -> Option<&str> {
        self.args.get(name).map(String::as_str)
    }

    /// The value of one argument, or the empty string.
    ///
    /// A command whose argument is the whole line — a title — treats a missing
    /// one and an empty one the same way, and this keeps that from being a
    /// `unwrap_or_default` at every call site.
    #[must_use]
    pub fn arg_or_empty(&self, name: &str) -> &str {
        self.args.get(name).map_or("", String::as_str)
    }

    /// Whether the argument was given at all.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.args.contains_key(name)
    }
}

/// One row of a completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// What the row shows and what Tab inserts.
    pub text: String,
    /// The text Tab appends to the line, when it is not the whole candidate.
    pub completion: String,
    /// A word to the right of the row.
    pub note: String,
}

/// Every command this host knows.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    commands: Vec<CommandSpec>,
}

impl Registry {
    /// An empty registry. A host fills one at startup, and a plugin adds to it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a command.
    ///
    /// A name that is already taken is handed over, not stolen: the command that
    /// held it keeps everything else it answers to. Taking the whole command
    /// away because one alias collided would let a plugin delete a command it
    /// never named, and these commands do share aliases with the ones they
    /// replace.
    pub fn register(&mut self, spec: CommandSpec) {
        let claimed: Vec<String> = spec.spellings().into_iter().map(str::to_owned).collect();
        let mut kept: Vec<CommandSpec> = Vec::with_capacity(self.commands.len() + 1);
        for mut existing in self.commands.drain(..) {
            existing.aliases.retain(|alias| !claimed.contains(alias));
            // What is left is dropped only when it has no name of its own left:
            // a command whose name was the one taken has nothing to answer to.
            if claimed.contains(&existing.name) {
                continue;
            }
            kept.push(existing);
        }
        kept.push(spec);
        self.commands = kept;
    }

    /// Every command, in declaration order.
    #[must_use]
    pub fn commands(&self) -> &[CommandSpec] {
        &self.commands
    }

    /// The command a name or alias names.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&CommandSpec> {
        self.commands
            .iter()
            .find(|spec| spec.spellings().contains(&name))
    }

    /// Parses one line.
    ///
    /// `None` means the text is not a command and belongs to whoever asked:
    /// only text that starts with a known name reaches the parser at all, so a
    /// message beginning with `/` still gets through. `Some(Err(_))` is a
    /// command whose arguments do not fit, and the caller reports it rather
    /// than sending it to the model.
    #[must_use]
    pub fn parse(&self, text: &str) -> Option<Result<Invocation, String>> {
        let text = text.trim();
        let (spec, rest) = self.split(text)?;
        Some(match assign(spec, rest) {
            Ok(args) => Ok(Invocation {
                command: spec.name.clone(),
                args,
            }),
            Err(problem) => Err(format!("{problem}\nusage: {}", spec.usage())),
        })
    }

    /// The command whose name this text starts with, and what follows it.
    ///
    /// The name has to end at a word boundary: `/reasoning` is a command and
    /// `/reasoningx` is not, and a parser that took the longer prefix would
    /// swallow a message that only looks like one.
    fn split<'a>(&self, text: &'a str) -> Option<(&CommandSpec, &'a str)> {
        self.commands.iter().find_map(|spec| {
            spec.spellings().into_iter().find_map(|spelling| {
                let rest = text.strip_prefix(spelling)?;
                (rest.is_empty() || rest.starts_with(char::is_whitespace)).then_some((spec, rest))
            })
        })
    }

    /// What may be typed next.
    ///
    /// With nothing but a slash it is the command list; past a command name it
    /// is the values of the argument the line is at. The list is what a person
    /// reads while typing, so it is filtered by what is already there and never
    /// contains anything already given.
    #[must_use]
    pub fn candidates(&self, text: &str, cursor: usize) -> Vec<Candidate> {
        if cursor != text.len() || !text.starts_with('/') {
            return Vec::new();
        }
        let Some(space) = text.find(char::is_whitespace) else {
            return self.command_candidates(text);
        };
        let Some(spec) = self.get(&text[..space]) else {
            return Vec::new();
        };
        let rest = &text[space..];
        let (completed, partial) = split_partial(rest);
        let filled = fill(spec, &completed);
        let Some(index) = next_arg(spec, &filled) else {
            return Vec::new();
        };
        let arg = &spec.args[index];
        let values = match &arg.values {
            Values::Closed(values) | Values::OpenWith(values) => values,
            Values::Open | Values::Rest => return Vec::new(),
        };
        values
            .iter()
            .filter(|value| completion_of(value).starts_with(&partial.to_ascii_lowercase()))
            .map(|value| {
                let target = completion_of(value);
                Candidate {
                    text: value.clone(),
                    completion: target[partial.len().min(target.len())..].to_owned(),
                    note: arg.name.clone(),
                }
            })
            .collect()
    }

    /// The commands a leading prefix names.
    fn command_candidates(&self, text: &str) -> Vec<Candidate> {
        let prefix = text.to_ascii_lowercase();
        let mut candidates = Vec::new();
        for spec in &self.commands {
            for spelling in spec.spellings() {
                if !spelling.to_ascii_lowercase().starts_with(&prefix) {
                    continue;
                }
                // The first spelling that matches is the one the row completes
                // to; the rest are named in the note so a person who knows the
                // command by its alias can see it is the same one.
                let others: Vec<&str> = spec
                    .spellings()
                    .into_iter()
                    .filter(|other| *other != spelling)
                    .collect();
                let mut note = spec.summary.clone();
                if !others.is_empty() {
                    let _ = write!(note, "  ({})", others.join(", "));
                }
                candidates.push(Candidate {
                    text: spelling.to_owned(),
                    completion: spelling[prefix.len().min(spelling.len())..].to_owned(),
                    note,
                });
            }
        }
        candidates
    }
}

/// The text of a closed value, without its template.
///
/// `budget:<tokens>` is typed as `budget:` and a number; completing the whole
/// thing would put `<tokens>` on the line.
fn completion_of(value: &str) -> String {
    match value.split_once('<') {
        Some((prefix, _)) => prefix.to_ascii_lowercase(),
        None => value.to_ascii_lowercase(),
    }
}

/// Splits the arguments of a line into the tokens that are complete and the one
/// still being typed.
fn split_partial(rest: &str) -> (Vec<String>, String) {
    let ends_with_space = rest.ends_with(char::is_whitespace);
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    if ends_with_space {
        (
            tokens.iter().map(|token| (*token).to_owned()).collect(),
            String::new(),
        )
    } else {
        let mut completed: Vec<String> = tokens
            .iter()
            .take(tokens.len().saturating_sub(1))
            .map(|token| (*token).to_owned())
            .collect();
        let partial = tokens
            .last()
            .map_or("", |token| *token)
            .to_ascii_lowercase();
        // A line that is only the command and its space has nothing partial;
        // the space is what said the argument is coming.
        if rest.trim().is_empty() {
            completed.clear();
        }
        (completed, partial)
    }
}

/// Which arguments a list of tokens fills.
fn fill(spec: &CommandSpec, tokens: &[String]) -> BTreeMap<String, String> {
    let mut filled = BTreeMap::new();
    for token in tokens {
        if let Some(index) = arg_for_token(spec, &filled, token) {
            let arg = &spec.args[index];
            if matches!(arg.values, Values::Rest) {
                // The rest swallows what is left and the loop is done: there is
                // nothing after it that could be anything but part of it.
                filled.insert(arg.name.clone(), token.clone());
                continue;
            }
            filled.insert(arg.name.clone(), token.clone());
        }
    }
    filled
}

/// Every word of a line, with the byte it starts at.
///
/// The offset is what lets the rest of the line be taken literally rather than
/// reassembled from its words.
fn tokens_of(text: &str) -> Vec<(usize, &str)> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some(from) = start.take() {
                tokens.push((from, &text[from..index]));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(from) = start {
        tokens.push((from, &text[from..]));
    }
    tokens
}

/// Which argument a token fills, if any.
///
/// A closed value wins over an open argument: `session` is the scope of
/// `/reasoning` even when the setting beside it has not been given yet, and
/// that is the whole of the ordering rule.
fn arg_for_token(
    spec: &CommandSpec,
    filled: &BTreeMap<String, String>,
    token: &str,
) -> Option<usize> {
    let open = |arg: &ArgSpec| {
        !filled.contains_key(&arg.name)
            && matches!(
                arg.values,
                Values::Open | Values::OpenWith(_) | Values::Rest
            )
    };
    if let Some(index) = spec.args.iter().position(|arg| {
        !filled.contains_key(&arg.name)
            && match &arg.values {
                Values::Closed(values) | Values::OpenWith(values) => values
                    .iter()
                    .any(|value| value_matches(value, &token.to_ascii_lowercase())),
                Values::Open | Values::Rest => false,
            }
    }) {
        return Some(index);
    }
    spec.args.iter().position(open)
}

/// Whether a token is a value of a closed argument.
fn value_matches(value: &str, token: &str) -> bool {
    match value.split_once('<') {
        // A template takes anything that starts with it, which is what makes
        // `budget:4096` the `budget:<tokens>` value with its number.
        Some((prefix, _)) => token.starts_with(&prefix.to_ascii_lowercase()),
        None => value.to_ascii_lowercase() == token,
    }
}

/// The index of the first argument nobody has filled, if any is left.
fn next_arg(spec: &CommandSpec, filled: &BTreeMap<String, String>) -> Option<usize> {
    spec.args
        .iter()
        .position(|arg| !filled.contains_key(&arg.name))
}

/// Assigns the tokens of a line to the arguments of a command.
///
/// A token nobody wants is a usage error rather than a silently dropped word: a
/// person who typed `/reasoning xhigh song` meant something by `song`, and
/// running the command without it is worse than saying it was not understood.
fn assign(spec: &CommandSpec, rest: &str) -> Result<BTreeMap<String, String>, String> {
    let mut filled = BTreeMap::new();
    let trimmed = rest.trim();
    for (offset, token) in tokens_of(trimmed) {
        let Some(index) = arg_for_token(spec, &filled, token) else {
            return Err(format!("`{}` has no `{token}`", spec.name));
        };
        let arg = &spec.args[index];
        if matches!(arg.values, Values::Rest) {
            // The rest is the line from where its first word starts, taken
            // whole. Joining the tokens back would collapse the two spaces
            // somebody put in the middle of a title, and a title with one space
            // is a different title.
            filled.insert(arg.name.clone(), trimmed[offset..].to_owned());
            return match missing(spec, &filled) {
                Some(arg) => Err(format!("`{}` needs {}", spec.name, arg.name)),
                None => Ok(filled),
            };
        }
        filled.insert(arg.name.clone(), token.to_owned());
    }
    match missing(spec, &filled) {
        Some(arg) => Err(format!("`{}` needs {}", spec.name, arg.name)),
        None => Ok(filled),
    }
}

/// The first required argument nobody filled.
fn missing<'a>(spec: &'a CommandSpec, filled: &BTreeMap<String, String>) -> Option<&'a ArgSpec> {
    spec.args
        .iter()
        .find(|arg| arg.required && !filled.contains_key(&arg.name))
}

#[cfg(test)]
mod tests {
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
        registry
            .register(CommandSpec::new("/rename", "name the session").arg(ArgSpec::rest("title")));
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
            refused.contains("usage: /allow <tool> [session|global]"),
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
}
