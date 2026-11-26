use crate::client::PluginOp;
use norte_code_client::PluginSummary;

pub(super) fn parse(args: &[&str]) -> Result<PluginOp, ()> {
    match args {
        ["install", url] if !url.is_empty() => Ok(PluginOp::InstallAll {
            url: (*url).to_owned(),
        }),
        ["install", url, names] if !url.is_empty() => {
            let packages: Vec<String> =
                names.split(',').map(str::trim).map(str::to_owned).collect();
            if packages.iter().any(String::is_empty) {
                return Err(());
            }
            if packages.len() == 1 {
                Ok(PluginOp::Install {
                    url: (*url).to_owned(),
                    package: packages.into_iter().next(),
                })
            } else {
                Ok(PluginOp::InstallMany {
                    url: (*url).to_owned(),
                    packages,
                })
            }
        }
        ["enable", name] => Ok(PluginOp::SetEnabled {
            name: (*name).to_owned(),
            enabled: true,
        }),
        ["disable", name] => Ok(PluginOp::SetEnabled {
            name: (*name).to_owned(),
            enabled: false,
        }),
        ["remove", name] => Ok(PluginOp::Remove {
            name: (*name).to_owned(),
        }),
        ["update"] => Ok(PluginOp::Update { name: None }),
        ["update", name] => Ok(PluginOp::Update {
            name: Some((*name).to_owned()),
        }),
        _ => Err(()),
    }
}

pub(super) fn suggestions(input: &str, plugins: &[PluginSummary]) -> Vec<String> {
    let Some(args) = input.strip_prefix("/plugin ") else {
        return Vec::new();
    };
    if !args.contains(' ') {
        return ["list", "enable", "disable", "install", "update", "remove"]
            .into_iter()
            .filter(|action| action.starts_with(args))
            .map(|action| format!("/plugin {action} "))
            .collect();
    }
    let Some((action, prefix)) = args.split_once(' ') else {
        return Vec::new();
    };
    if !matches!(action, "enable" | "disable" | "update" | "remove") || prefix.contains(' ') {
        return Vec::new();
    }
    plugins
        .iter()
        .filter(|plugin| match action {
            "enable" => !plugin.enabled,
            "disable" => plugin.enabled,
            _ => true,
        })
        .filter(|plugin| plugin.name.starts_with(prefix))
        .map(|plugin| format!("/plugin {action} {}", plugin.name))
        .collect()
}

#[cfg(test)]
#[path = "plugin_command_tests.rs"]
mod tests;
