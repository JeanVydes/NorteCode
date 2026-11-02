//! The completion menu as a widget.

use crate::style::{Style, icon, metrics};
use crate::ui::clip;
use crate::ui::Widget;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _};

pub(crate) struct CompletionMenu<'a> {
    pub(crate) rows: &'a [String],
    pub(crate) cursor: usize,
    pub(crate) top: usize,
    pub(crate) style: Style,
}

impl Widget for CompletionMenu<'_> {
    fn measure(&self, _width: u16, _theme: &crate::style::Theme) -> u16 {
        self.rows.len().min(metrics::COMPLETION_ROWS) as u16
    }

    fn draw(
        &self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        _theme: &crate::style::Theme,
    ) {
        if area.height == 0 {
            return;
        }
        let take = usize::from(area.height).min(metrics::COMPLETION_ROWS);
        for offset in 0..take {
            let index = self.top + offset;
            let Some(row) = self.rows.get(index) else { break };
            let here = index == self.cursor;
            let row = row.replace('\n', " ");
            let room = usize::from(area.width)
                .saturating_sub(metrics::MENU_INDENT + 2)
                .max(1);
            let shown = clip::clip_text(&row, room);
            let text = format!("{}{shown}", " ".repeat(metrics::MENU_INDENT));
            let line = if here {
                Line::styled(text, Style::selected())
            } else {
                Line::from(Span::styled(text, self.style.dim_span()))
            };
            let row_area = Rect::new(area.x, area.y + offset as u16, area.width, 1);
            Paragraph::new(line).render(row_area, frame.buffer_mut());
        }
    }
}
EOF
python3 - <<'PY'
p='norte-code-cli/src/ui/mod.rs'
s=open(p).read()
if 'pub(crate) mod completion;' not in s:
    s=s.replace('pub(crate) mod clip;\n', 'pub(crate) mod clip;\npub(crate) mod completion;\n',1)
open(p,'w').write(s)
PY
# truncate
python3 - <<'PY'
p='norte-code-cli/src/ui/completion.rs'
lines=open(p).read().splitlines()
out=[]
for l in lines:
    if l=='EOF' or l.startswith('echo '):
        break
    out.append(l)
open(p,'w').write('\n'.join(out)+'\n')
PY
cargo build -p norte-code-cli 2>&1 | tail -4