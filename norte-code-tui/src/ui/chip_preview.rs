//! Bounded local thumbnails and hover cards for composer attachments.
use crate::{
    attachment::{AttachmentKind, AttachmentSource, PreparedAttachment},
    chip::ChipId,
};
use base64::Engine;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap},
};
use std::{
    collections::HashMap,
    io::{Cursor, Write},
    sync::Arc,
};
use terminalui::Theme;

pub(super) struct Thumbnail {
    pixels: image::RgbImage,
    png: Arc<Vec<u8>>,
    size: (u32, u32),
}

impl Thumbnail {
    pub(super) fn from_attachment(attachment: &PreparedAttachment) -> Option<Self> {
        if attachment.kind != AttachmentKind::Image {
            return None;
        }
        let mut reader = match &attachment.source {
            AttachmentSource::Image(bytes) => {
                image::ImageReader::new(Cursor::new(bytes.as_slice()))
                    .with_guessed_format()
                    .ok()?
            }
            AttachmentSource::File(path) => {
                if std::fs::metadata(path).ok()?.len() > 16 * 1024 * 1024 {
                    return None;
                }
                return Self::decode(
                    image::ImageReader::open(path)
                        .ok()?
                        .with_guessed_format()
                        .ok()?,
                );
            }
        };
        reader.set_format(image::ImageFormat::Png);
        Self::decode(reader)
    }

    fn decode<R: std::io::BufRead + std::io::Seek>(
        mut reader: image::ImageReader<R>,
    ) -> Option<Self> {
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(64 * 1024 * 1024);
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        reader.limits(limits);
        let original = reader.decode().ok()?;
        let size = (original.width(), original.height());
        let small = original.thumbnail(512, 320);
        let mut png = Cursor::new(Vec::new());
        small.write_to(&mut png, image::ImageFormat::Png).ok()?;
        Some(Self {
            pixels: small.thumbnail(64, 32).to_rgb8(),
            png: Arc::new(png.into_inner()),
            size,
        })
    }
}
#[derive(Default)]
pub(super) struct Preview {
    pub(super) pointer: Option<(u16, u16)>,
    pub(super) images: HashMap<ChipId, Thumbnail>,
    pub(crate) native: Option<(Rect, Arc<Vec<u8>>)>,
}
impl Preview {
    pub(super) fn paint(
        &mut self,
        id: ChipId,
        text: &str,
        composer: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let screen = buf.area;
        let width = screen.width.min(68);
        let height = composer.y.saturating_sub(screen.y).min(18);
        if width < 10 || height < 4 {
            return;
        }
        let x = self
            .pointer
            .map_or(composer.x, |p| p.0)
            .min(screen.right() - width);
        let area = Rect::new(x, composer.y - height, width, height);
        let thumb = self.images.get(&id);
        let title = thumb.map_or_else(
            || " Preview ".to_owned(),
            |t| format!(" Image   {} × {} ", t.size.0, t.size.1),
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(Style::default().bg(theme.secondary).fg(theme.accent));
        let inner = block.inner(area);
        Clear.render(area, buf);
        block.render(area, buf);
        if let Some(t) = thumb {
            let scale = (f64::from(inner.width) / f64::from(t.pixels.width()))
                .min(f64::from(inner.height * 2) / f64::from(t.pixels.height()));
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (w, h) = (
                (f64::from(t.pixels.width()) * scale) as u16,
                (f64::from(t.pixels.height()) * scale / 2.0) as u16,
            );
            let rect = Rect::new(inner.x, inner.y, w.max(1), h.max(1));
            for y in 0..rect.height {
                for x in 0..rect.width {
                    let color = |dy: u32| {
                        let pixel = t.pixels.get_pixel(
                            u32::from(x) * t.pixels.width() / u32::from(rect.width),
                            (2 * u32::from(y) + dy) * t.pixels.height()
                                / (2 * u32::from(rect.height)),
                        );
                        Color::Rgb(pixel[0], pixel[1], pixel[2])
                    };
                    buf[(rect.x + x, rect.y + y)]
                        .set_char('▀')
                        .set_fg(color(0))
                        .set_bg(color(1));
                }
            }
            self.native = Some((rect, Arc::clone(&t.png)));
        } else {
            Paragraph::new(text.chars().take(8192).collect::<String>())
                .style(Style::default().fg(theme.foreground))
                .wrap(Wrap { trim: false })
                .render(inner, buf);
        }
    }
}

#[derive(Default)]
pub(crate) struct KittyPlacement(Option<(Rect, Arc<Vec<u8>>)>);
impl KittyPlacement {
    pub(crate) fn sync(
        &mut self,
        next: Option<(Rect, Arc<Vec<u8>>)>,
        out: &mut impl Write,
    ) -> std::io::Result<()> {
        if self
            .0
            .as_ref()
            .zip(next.as_ref())
            .is_some_and(|(a, b)| a.0 == b.0 && Arc::ptr_eq(&a.1, &b.1))
        {
            return Ok(());
        }
        if self.0.take().is_some() {
            out.write_all(b"\x1b_Ga=d,d=I,i=732091,q=2\x1b\\")?;
        }
        if let Some((rect, png)) = &next {
            let encoded = base64::engine::general_purpose::STANDARD.encode(png.as_slice());
            write!(out, "\x1b7\x1b[{};{}H", rect.y + 1, rect.x + 1)?;
            let mut chunks = encoded.as_bytes().chunks(4096).peekable();
            let mut first = true;
            while let Some(chunk) = chunks.next() {
                out.write_all(b"\x1b_G")?;
                if first {
                    write!(
                        out,
                        "a=T,f=100,i=732091,q=2,C=1,c={},r={},",
                        rect.width, rect.height
                    )?;
                    first = false;
                }
                write!(out, "m={};", u8::from(chunks.peek().is_some()))?;
                out.write_all(chunk)?;
                out.write_all(b"\x1b\\")?;
            }
            out.write_all(b"\x1b8")?;
        }
        self.0 = next;
        out.flush()
    }
}
