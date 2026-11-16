use super::*;

struct Upper;

impl StreamTransform for Upper {
    fn name(&self) -> &'static str {
        "upper"
    }

    fn transform(&self, delta: StreamDelta) -> Vec<StreamDelta> {
        match delta {
            StreamDelta::Text { index, text } => vec![StreamDelta::Text {
                index,
                text: text.to_uppercase(),
            }],
            other => vec![other],
        }
    }
}

struct DropText;

impl StreamTransform for DropText {
    fn name(&self) -> &'static str {
        "drop-text"
    }

    fn transform(&self, delta: StreamDelta) -> Vec<StreamDelta> {
        match delta {
            StreamDelta::Text { .. } => Vec::new(),
            other => vec![other],
        }
    }
}

#[test]
fn transforms_project_deltas_in_registration_order() {
    let set =
        ObserverSet::with_transforms(Vec::new(), vec![Arc::new(DropText), Arc::new(Upper)], 4);
    assert_eq!(set.transform_names(), vec!["drop-text", "upper"]);
    let text = StreamDelta::Text {
        index: 0,
        text: "hi".to_owned(),
    };
    assert!(
        set.project(text).is_empty(),
        "a dropped delta projects nothing"
    );
}

#[test]
fn a_transform_annotates_without_touching_other_deltas() {
    let set = ObserverSet::with_transforms(Vec::new(), vec![Arc::new(Upper)], 4);
    let out = set.project(StreamDelta::Text {
        index: 0,
        text: "hi".to_owned(),
    });
    assert_eq!(
        out,
        vec![StreamDelta::Text {
            index: 0,
            text: "HI".to_owned()
        }]
    );
    let continuity = StreamDelta::Continuity { index: 0, len: 3 };
    assert_eq!(set.project(continuity.clone()), vec![continuity]);
}
