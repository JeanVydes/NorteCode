use super::*;

#[test]
fn chart_config_clamps_and_maps_closed_dot_sizes() {
    let config = norte_code_protocol::config::ChartConfig {
        contrast: u8::MAX,
        density: 0,
        dot_size: norte_code_protocol::config::ChartDotSize::Large,
    };
    let mapped = HalftoneConfig::from_chart_config(&config);
    assert_eq!(mapped.contrast, 1.0);
    assert_eq!(mapped.density, 0.0);
    assert_eq!(mapped.dot_size, DotSize::Large);
}

#[test]
fn point_in_polygon_handles_a_triangle() {
    let triangle = [(0.0, 0.0), (4.0, 0.0), (2.0, 4.0)];
    assert!(inside(2.0, 1.0, &triangle));
    assert!(!inside(0.0, 4.0, &triangle));
}
