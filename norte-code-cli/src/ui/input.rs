//! The input block as a widget seam.

use crate::ui::Widget;
use ratatui::layout::Rect;

#[allow(dead_code)]
pub(crate) struct InputBlock;

impl Widget for InputBlock {
    fn measure(&self, _width: u16, _theme: &crate::style::Theme) -> u16 {
        1
    }

    fn draw(
        &self,
        _frame: &mut ratatui::Frame<'_>,
        _area: Rect,
        _theme: &crate::style::Theme,
    ) {
    }
}
EOF
python3 - <<'PY'
p='norte-code-cli/src/ui/mod.rs'
s=open(p).read()
if 'pub(crate) mod input;' not in s:
    s=s.replace('pub(crate) mod completion;\n', 'pub(crate) mod completion;\npub(crate) mod input;\n',1)
open(p,'w').write(s)
PY
python3 - <<'PY'
p='norte-code-cli/src/ui/input.rs'
lines=open(p).read().splitlines()
out=[]
for l in lines:
    if l=='EOF' or l.startswith('echo '):
        break
    out.append(l)
open(p,'w').write('\n'.join(out)+'\n')
PY
cargo build -p norte-code-cli 2>&1 | tail -3