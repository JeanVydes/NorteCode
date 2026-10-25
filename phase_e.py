"""Import Phase by name and use it unqualified in the frame tests."""

import pathlib

path = pathlib.Path("norte-code-cli/src/render/tests.rs")
text = path.read_text()

OLD = "use crate::render::loading::{LOADING_BAR_WIDTH, loading_frame};"
NEW = "use crate::render::loading::{LOADING_BAR_WIDTH, Phase, loading_frame};"
assert text.count(OLD) == 1, text.count(OLD)
text = text.replace(OLD, NEW)

text = text.replace("loading::Phase::Waiting", "Phase::Waiting")

path.write_text(text)
print("ok")
