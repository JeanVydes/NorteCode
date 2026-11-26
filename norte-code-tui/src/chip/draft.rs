//! The composer's text together with its atomic chips.

use norte_code_wire::{UploadReceipt, UserInputPart};
use serde::Serialize;

use super::content::OriginalText;
use super::label::ChipLabel;
use super::unit::{Chip, ChipId};
use crate::attachment::AttachmentKind;

/// One piece of the displayed draft.
#[derive(Clone, Debug, Serialize)]
pub(crate) enum Segment {
    /// Ordinary typed text.
    Text(String),
    /// One atomic chip.
    Chip(Chip),
}

/// Where a chip label sits in the displayed text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ChipSpan {
    /// The chip occupying this range.
    pub(crate) id: ChipId,
    /// First character of the label.
    pub(crate) start: usize,
    /// One past the last character of the label.
    pub(crate) end: usize,
}

/// The current composer content.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Draft {
    segments: Vec<Segment>,
    display: String,
    spans: Vec<ChipSpan>,
    next_id: u64,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            segments: Vec::new(),
            display: String::new(),
            spans: Vec::new(),
            next_id: 1,
        }
    }
}

impl Draft {
    /// The text as drawn by the input.
    pub(crate) fn text(&self) -> &str {
        &self.display
    }

    /// Where the chip labels sit in the displayed text.
    pub(crate) fn spans(&self) -> &[ChipSpan] {
        &self.spans
    }

    /// A chip by its client identity.
    pub(crate) fn chip(&self, id: ChipId) -> Option<&Chip> {
        self.segments.iter().find_map(|segment| match segment {
            Segment::Chip(chip) if chip.id() == id => Some(chip),
            _ => None,
        })
    }

    /// The number of characters in the displayed text.
    pub(crate) fn len(&self) -> usize {
        self.display.chars().count()
    }

    /// Move a caret out of the inside of a chip.
    pub(crate) fn normalize_index(&self, index: usize) -> usize {
        for span in &self.spans {
            if index > span.start && index < span.end {
                return span.end;
            }
        }
        index.min(self.len())
    }

    /// The chip label under a caret, when there is one.
    pub(crate) fn chip_range_at(&self, index: usize) -> Option<(usize, usize)> {
        self.spans
            .iter()
            .find(|span| index >= span.start && index <= span.end)
            .map(|span| (span.start, span.end))
    }

    /// Insert ordinary text, replacing any chips the range touches.
    pub(crate) fn replace_range(&mut self, start: usize, end: usize, insert: &str) -> usize {
        let (start, end) = self.expand_range(start, end);
        let (start_segment, start_offset) = self.locate(start);
        let (end_segment, end_offset) = self.locate(end);

        let mut next = Vec::new();
        for segment in self.segments.iter().take(start_segment) {
            next.push(segment.clone());
        }
        push_text_prefix(&mut next, self.segments.get(start_segment), start_offset);
        if !insert.is_empty() {
            next.push(Segment::Text(insert.to_owned()));
        }
        push_text_suffix(&mut next, self.segments.get(end_segment), end_offset);
        let tail = if matches!(self.segments.get(end_segment), Some(Segment::Chip(_))) {
            end_segment
        } else {
            end_segment + 1
        };
        for segment in self.segments.iter().skip(tail) {
            next.push(segment.clone());
        }

        self.segments = merge_text(next);
        self.rebuild();
        start + insert.chars().count()
    }

    /// Turn a long paste into one atomic chip.
    pub(crate) fn insert_text_chip(&mut self, index: usize, text: OriginalText) -> usize {
        let id = self.next_chip_id();
        self.insert_chip(index, Chip::text(id, text))
    }

    pub(crate) fn insert_pending(
        &mut self,
        attachment: crate::attachment::PreparedAttachment,
    ) -> usize {
        let id = self.next_chip_id();
        self.insert_chip(attachment.cursor, Chip::pending(id, attachment))
    }

    pub(crate) fn pending_attachments(&self) -> Vec<crate::attachment::PreparedAttachment> {
        self.segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Chip(chip) => chip.pending_attachment().cloned(),
                Segment::Text(_) => None,
            })
            .collect()
    }

    /// Add an uploaded artifact as one atomic chip.
    pub(crate) fn insert_upload(
        &mut self,
        index: usize,
        kind: AttachmentKind,
        receipt: UploadReceipt,
    ) -> usize {
        let id = self.next_chip_id();
        let label = ChipLabel::uploaded(kind, 1);
        self.insert_chip(index, Chip::upload(id, kind, receipt, label))
    }

    /// Remove every chip and all text, keeping identifier numbers unique.
    pub(crate) fn clear(&mut self) {
        self.segments.clear();
        self.rebuild();
    }

    /// The text with every chip replaced by what the message really carries.
    pub(crate) fn expanded_text(&self) -> String {
        self.expanded_range(0, self.len())
    }

    /// The expanded text covered by a display range.
    pub(crate) fn expanded_range(&self, start: usize, end: usize) -> String {
        let (start, end) = self.expand_range(start, end);
        let chars = self.display.chars().collect::<Vec<_>>();
        let mut out = String::new();
        let mut position = start;
        for span in self
            .spans
            .iter()
            .filter(|span| span.end > start && span.start < end)
        {
            if span.start > position {
                out.extend(chars[position..span.start].iter());
            }
            let Some(chip) = self.chip(span.id) else {
                position = span.end;
                continue;
            };
            out.push_str(&chip.expanded_text());
            position = span.end;
        }
        if position < end {
            out.extend(chars[position..end].iter());
        }
        out
    }

    /// The upload parts the message must carry.
    pub(crate) fn wire_parts(&self) -> Vec<UserInputPart> {
        self.segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Chip(chip) => chip.wire_part(),
                Segment::Text(_) => None,
            })
            .collect()
    }

    fn insert_chip(&mut self, index: usize, chip: Chip) -> usize {
        let point = self.normalize_index(index);
        let id = chip.id();
        let (segment, offset) = self.locate(point);

        let mut next = Vec::new();
        for part in self.segments.iter().take(segment) {
            next.push(part.clone());
        }
        push_text_prefix(&mut next, self.segments.get(segment), offset);
        next.push(Segment::Chip(chip));
        push_text_suffix(&mut next, self.segments.get(segment), offset);
        for part in self.segments.iter().skip(segment + 1) {
            next.push(part.clone());
        }

        self.segments = merge_text(next);
        self.rebuild();
        self.spans
            .iter()
            .find(|span| span.id == id)
            .map_or(point, |span| span.end)
    }

    fn next_chip_id(&mut self) -> ChipId {
        let id = ChipId::new(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    fn expand_range(&self, start: usize, end: usize) -> (usize, usize) {
        let mut start = start;
        let mut end = end.max(start);
        for span in &self.spans {
            if span.end > start && span.start < end {
                start = start.min(span.start);
                end = end.max(span.end);
            }
        }
        (start, end)
    }

    fn locate(&self, index: usize) -> (usize, usize) {
        let mut position = 0;
        for (cursor, segment) in self.segments.iter().enumerate() {
            let width = segment_width(segment);
            if index < position + width {
                return (cursor, index - position);
            }
            if index == position + width {
                return (cursor + 1, 0);
            }
            position += width;
        }
        (self.segments.len(), 0)
    }

    fn rebuild(&mut self) {
        let mut counts = [0usize; 4];
        for segment in &mut self.segments {
            let Segment::Chip(chip) = segment else {
                continue;
            };
            let Some(kind) = chip.kind().attachment() else {
                continue;
            };
            let slot = kind.slot();
            counts[slot] += 1;
            chip.set_label(ChipLabel::uploaded(kind, counts[slot]));
        }

        let mut display = String::new();
        let mut spans = Vec::new();
        for segment in &self.segments {
            match segment {
                Segment::Text(text) => display.push_str(text),
                Segment::Chip(chip) => {
                    let start = display.chars().count();
                    display.push_str(chip.label().as_str());
                    spans.push(ChipSpan {
                        id: chip.id(),
                        start,
                        end: display.chars().count(),
                    });
                }
            }
        }
        self.display = display;
        self.spans = spans;
    }
}

fn segment_width(segment: &Segment) -> usize {
    match segment {
        Segment::Text(text) => text.chars().count(),
        Segment::Chip(chip) => chip.label().as_str().chars().count(),
    }
}

fn push_text_prefix(out: &mut Vec<Segment>, segment: Option<&Segment>, offset: usize) {
    if let Some(Segment::Text(text)) = segment {
        let prefix = text.chars().take(offset).collect::<String>();
        if !prefix.is_empty() {
            out.push(Segment::Text(prefix));
        }
    }
}

fn push_text_suffix(out: &mut Vec<Segment>, segment: Option<&Segment>, offset: usize) {
    if let Some(Segment::Text(text)) = segment {
        let suffix = text.chars().skip(offset).collect::<String>();
        if !suffix.is_empty() {
            out.push(Segment::Text(suffix));
        }
    }
}

fn merge_text(segments: Vec<Segment>) -> Vec<Segment> {
    let mut merged: Vec<Segment> = Vec::new();
    for segment in segments {
        if let (Some(Segment::Text(previous)), Segment::Text(current)) =
            (merged.last_mut(), &segment)
        {
            previous.push_str(current);
            continue;
        }
        merged.push(segment);
    }
    merged
}
