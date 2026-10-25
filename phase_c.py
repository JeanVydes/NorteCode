"""Bring the bar back after a tool, so the rest of the turn is not silent.

The bar stops on the first answer token, which is right; but a turn that called
a tool goes on after it, and the model can take many seconds to continue. From
the moment the first tool ran there was no signal at all: a turn that called
`write` and then thought for twelve seconds looked dead.
"""

import pathlib

path = pathlib.Path("norte-code-cli/src/render/renderer.rs")
text = path.read_text()

OLD = """                if *is_error && tool.as_str() != "edit" {
                    // A failure needs its reason whatever the verbosity.
                    if let Some(error) = error {
                        self.print_tool_result(error, *truncated);
                    }
                } else if let Some(result) =
                    Self::tool_result(tool, path.as_deref(), output.as_deref())
                {
                    self.print_tool_result(&result, *truncated);
                }"""

NEW = """                if *is_error && tool.as_str() != "edit" {
                    // A failure needs its reason whatever the verbosity.
                    if let Some(error) = error {
                        self.print_tool_result(error, *truncated);
                    }
                } else if let Some(result) =
                    Self::tool_result(tool, path.as_deref(), output.as_deref())
                {
                    self.print_tool_result(&result, *truncated);
                }
                // The turn is not over: the model has the result and is being
                // asked again. Without this the rest of the turn ran with no
                // signal at all, and a slow continuation looked like a hang.
                if self.loading_bar.is_none() && !self.failed && !self.cancelled {
                    self.mark_working();
                }"""

assert text.count(OLD) == 1, text.count(OLD)
path.write_text(text.replace(OLD, NEW))
print("ok")
