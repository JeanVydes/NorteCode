use norte_code_wire::Usage;

use super::fold_request_usage;
use crate::ui_model::UiModel;

#[test]
fn request_usage_saturates_instead_of_overflowing_the_context_size() {
    let usage = Usage {
        input_tokens: Some(u64::MAX),
        output_tokens: Some(u64::MAX),
        ..Usage::default()
    };
    let mut model = UiModel::detached();

    fold_request_usage(&usage, &mut model);

    assert_eq!(model.context.used, u64::MAX);
}
