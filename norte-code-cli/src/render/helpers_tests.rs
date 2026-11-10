use super::{cache_percent_label, percent_label};

#[test]
fn the_cache_share_always_carries_two_decimals() {
    // A cache that stops hitting moves by hundredths, so the hundredths are
    // the point; an exact and an empty share read the same way.
    assert_eq!(cache_percent_label(9999, 10_000), "99.99%");
    assert_eq!(cache_percent_label(9, 10), "90.00%");
    assert_eq!(cache_percent_label(10, 10), "100.00%");
    assert_eq!(cache_percent_label(0, 10), "0.00%");
    assert_eq!(cache_percent_label(5, 0), "0.00%");
}

#[test]
fn the_context_share_keeps_its_own_format() {
    // The dial is a different figure: it does not move by hundredths, so it
    // keeps the shorter form.
    assert_eq!(percent_label(1, 100), "1.0%");
    assert_eq!(percent_label(90, 100), "90%");
    assert_eq!(percent_label(1, 0), "0%");
}
