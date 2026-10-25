"""Tell the bar when the provider started answering.

The first delta of any kind is the signal: nothing is emitted before it, so its
arrival is what turns "waiting for the first token" into "streaming".
"""

import pathlib

path = pathlib.Path("norte-code-cli/src/render/renderer.rs")
text = path.read_text()


def sub(old, new, why, count=1):
    global text
    found = text.count(old)
    assert found == count, (why, found, count)
    text = text.replace(old, new)


# A reasoning delta is the provider answering.
sub(
    """                if self.verbosity.is_quiet() {
                    // `low` is the answer alone: not even the fact that the
                    // model is thinking is shown.
                } else if !self.verbosity.at_least(Verbosity::High) {""",
    """                // The provider answered: the bar stops saying it is waiting.
                if let Some(bar) = self.loading_bar.as_ref() {
                    bar.streaming();
                }
                if self.verbosity.is_quiet() {
                    // `low` is the answer alone: not even the fact that the
                    // model is thinking is shown.
                } else if !self.verbosity.at_least(Verbosity::High) {""",
    "reasoning",
)

# And so is an answer delta.
sub(
    """    pub(crate) fn answer_delta(&mut self, text: &str) {
        let text = crate::text::terminal(text);""",
    """    pub(crate) fn answer_delta(&mut self, text: &str) {
        let text = crate::text::terminal(text);
        // The first answer token is the same signal as the first reasoning one.
        if !text.is_empty() {
            if let Some(bar) = self.loading_bar.as_ref() {
                bar.streaming();
            }
        }""",
    "answer",
)

path.write_text(text)
print("ok")
