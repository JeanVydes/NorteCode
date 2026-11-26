use super::{parse, suggestions};
use crate::client::PluginOp;
use norte_code_client::PluginSummary;

#[test]
fn installs_a_single_package_or_a_comma_separated_list() {
    assert_eq!(
        parse(&["install", "https://example.test/repo"]),
        Ok(PluginOp::InstallAll {
            url: "https://example.test/repo".to_owned()
        })
    );
    assert_eq!(
        parse(&["install", "https://example.test/repo", "one"]),
        Ok(PluginOp::Install {
            url: "https://example.test/repo".to_owned(),
            package: Some("one".to_owned())
        })
    );
    assert_eq!(
        parse(&["install", "https://example.test/repo", "one,two"]),
        Ok(PluginOp::InstallMany {
            url: "https://example.test/repo".to_owned(),
            packages: vec!["one".to_owned(), "two".to_owned()]
        })
    );
    assert!(parse(&["install", "https://example.test/repo", "one,,two"]).is_err());
    assert!(parse(&["remove"]).is_err());
}

#[test]
fn completes_actions_and_eligible_installed_plugins() {
    let plugins = vec![
        PluginSummary {
            name: "alpha".into(),
            enabled: true,
            ..PluginSummary::default()
        },
        PluginSummary {
            name: "beta".into(),
            enabled: false,
            ..PluginSummary::default()
        },
    ];
    assert_eq!(suggestions("/plugin en", &plugins), ["/plugin enable "]);
    assert_eq!(
        suggestions("/plugin enable ", &plugins),
        ["/plugin enable beta"]
    );
    assert_eq!(
        suggestions("/plugin disable ", &plugins),
        ["/plugin disable alpha"]
    );
    assert_eq!(
        suggestions("/plugin remove a", &plugins),
        ["/plugin remove alpha"]
    );
}
