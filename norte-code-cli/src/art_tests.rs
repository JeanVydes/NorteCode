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
