"""Point the import at the crate root and update the frame tests."""

import pathlib


def sub(path, old, new, why, count=1):
    text = path.read_text()
    found = text.count(old)
    assert found == count, (why, found, count)
    path.write_text(text.replace(old, new))


sub(
    pathlib.Path("norte-code-cli/src/render/loading.rs"),
    """use super::Style;
use super::helpers::duration_label;""",
    """use super::Style;
use crate::duration_label;""",
    "import",
)

tests = pathlib.Path("norte-code-cli/src/render/tests.rs")
sub(
    tests,
    "    let frame = loading_frame(Style::plain(), 3);",
    "    let frame = loading_frame(Style::plain(), 3, loading::Phase::Waiting);",
    "one",
)
sub(
    tests,
    "        loading_frame(Style::plain(), 0),",
    "        loading_frame(Style::plain(), 0, loading::Phase::Waiting),",
    "two",
)
sub(
    tests,
    "        loading_frame(Style::plain(), 2)",
    "        loading_frame(Style::plain(), 2, loading::Phase::Waiting)",
    "three",
)
sub(
    tests,
    "    use crate::render::loading::{LOADING_BAR_WIDTH, loading_frame};",
    "    use crate::render::loading::{LOADING_BAR_WIDTH, Phase, loading_frame};",
    "test import",
)

print("ok")
