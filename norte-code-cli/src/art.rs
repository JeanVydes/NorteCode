//! The art: the wordmark, the mascot, and what moves.
//!
//! Kept apart from the drawing so a person can change a face without touching a
//! layout, and so the frames can be checked as text. Everything here is plain
//! characters with no colour of its own: the ink is the widget's, which is what
//! lets the same art read on a terminal with no colour at all.
//!
//! The mascot is a polar bear, because the name is Norte and the north is where
//! the bear is. It blinks and then winks, which is the whole of the animation:
//! something that moved constantly would be a thing to look at while a person
//! is trying to read a list of sessions.

/// How long one frame of an animation is shown, in ms.
///
/// Slow on purpose: this is a face in the corner of a screen somebody is
/// reading, and a faster one pulls the eye for no reason.
pub(crate) const FRAME_MS: u64 = 640;

/// The wordmark, one string per row.
///
/// Six rows, because the screen is a terminal of unknown height and the art is
/// the first thing that should give way.
pub(crate) const WORDMARK: [&str; 6] = [
    r"  ███╗   ██╗ ██████╗ ██████╗ ████████╗███████╗",
    r"  ████╗  ██║██╔═══██╗██╔══██╗╚══██╔══╝██╔════╝",
    r"  ██╔██╗ ██║██║   ██║██████╔╝   ██║   █████╗  ",
    r"  ██║╚██╗██║██║   ██║██╔══██╗   ██║   ██╔══╝  ",
    r"  ██║ ╚████║╚██████╔╝██║  ██║   ██║   ███████╗",
    r"  ╚═╝  ╚═══╝ ╚═════╝ ╚═╝  ╚═╝   ╚═╝   ╚══════╝",
];

/// The tagline under the wordmark.
pub(crate) const TAGLINE: &str = "the north of your code";

/// The small wordmark, for the top of the conversation.
///
/// One row rather than six: the transcript is where the work is, and the art
/// there is a marker that a session started, not a screen.
pub(crate) const MARK: &str = "❄ norte";

/// How wide the mascot is, in cells.
///
/// Every frame is exactly this wide, so nothing beside or under it moves when
/// the face does. The rows are padded to it rather than trusted to match.
const BEAR_WIDTH: usize = 19;

/// The head, which every frame shares.
///
/// The ears and the muzzle are the same in all three frames: a bear whose whole
/// head changed would be a different bear blinking.
const BEAR_HEAD: [&str; 6] = [
    "     ▄▄▄▄▄▄▄▄▄     ",
    "   ▄███████████▄   ",
    "  ███████████████  ",
    " █████████████████ ",
    "  ███████████████  ",
    "   ▀███████████▀   ",
];

/// The three faces, one per frame: awake, blinking, winking.
///
/// The eyes are drawn over the head, at the row and column they belong to. They
/// are the only thing that changes.
const EYES: [&str; 3] = [
    // Awake: both eyes open.
    "  ██  ●     ●  ██  ",
    // Blinking: both closed.
    "  ██  ─     ─  ██  ",
    // A wink: one open, one closed.
    "  ██  ●     ─  ██  ",
];

/// The muzzle, which none of the faces changes.
const MUZZLE: &str = "  ████  ▄▄▄  ████  ";

/// The frames of the mascot, in order.
///
/// Three poses and not a loop of many: a face that changed often would pull the
/// eye away from the list under it, and this is a screen somebody is reading.
#[must_use]
pub(crate) fn mascot(frame: usize) -> Vec<String> {
    let face = frame % EYES.len();
    let mut rows: Vec<String> = Vec::with_capacity(BEAR_HEAD.len());
    for (index, row) in BEAR_HEAD.iter().enumerate() {
        let line = match index {
            // The eyes sit on the third row of the head.
            2 => EYES[face],
            // The muzzle is under them.
            4 => MUZZLE,
            _ => row,
        };
        rows.push(pad(line));
    }
    rows
}

/// Puts a row at the width of the art, so every frame lines up.
fn pad(row: &str) -> String {
    let mut line: String = row.chars().take(BEAR_WIDTH).collect();
    while line.chars().count() < BEAR_WIDTH {
        line.push(' ');
    }
    line
}

/// How many columns the mascot takes, for centring it.
#[must_use]
pub(crate) fn mascot_width() -> usize {
    BEAR_WIDTH
}

/// How many columns the wordmark takes.
#[must_use]
pub(crate) fn wordmark_width() -> usize {
    WORDMARK
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_of_the_mascot_is_the_same_size() {
        // A frame of another size would move everything under it when it
        // blinked, which is the one thing an idle animation must not do.
        let first = mascot(0);
        assert_eq!(first.len(), BEAR_HEAD.len());
        for frame in 0..EYES.len() {
            let rows = mascot(frame);
            for (index, row) in rows.iter().enumerate() {
                assert_eq!(
                    row.chars().count(),
                    BEAR_WIDTH,
                    "frame {frame}, row {index}: {}",
                    row.chars().count()
                );
            }
        }
    }

    #[test]
    fn the_frames_are_not_all_the_same_face() {
        // An animation whose frames are identical is a still image with a
        // thread behind it.
        assert_ne!(mascot(0), mascot(1), "it blinks");
        assert_ne!(mascot(1), mascot(2), "and it winks");
        assert_eq!(mascot(3), mascot(0), "and it comes back around");
    }

    #[test]
    fn the_head_is_the_same_in_every_frame() {
        // Only the eyes move: a bear whose ears twitched would read as a
        // different drawing rather than as the same one blinking.
        let frames: Vec<Vec<String>> = (0..EYES.len()).map(mascot).collect();
        for row in [0, 1, 3, 4, 5] {
            let first = &frames[0][row];
            for frame in &frames {
                assert_eq!(&frame[row], first, "row {row} moved");
            }
        }
    }

    #[test]
    fn the_art_is_plain_text_with_nothing_to_escape() {
        // The art is drawn by the frame, which colours it: a string carrying an
        // escape here would be characters on screen, not ink.
        for row in WORDMARK.iter().chain(BEAR_HEAD.iter()) {
            assert!(!row.contains('\u{1b}'), "{row:?}");
        }
        for face in EYES {
            assert!(!face.contains('\u{1b}'), "{face:?}");
        }
    }
}
