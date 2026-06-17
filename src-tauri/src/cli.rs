use crate::{accounts, config, local_http_api, plugin_engine};
use std::path::{Path, PathBuf};

const APP_IDENTIFIER: &str = "com.sunstory.openusage";
const CLI_HELP: &str = "\
OpenUsage CLI

Usage:
  openusage usage [--no-refresh]
  openusage refresh

Commands:
  usage       Refresh enabled providers, then print cached usage JSON
  refresh     Alias for usage

Options:
  --no-refresh  Print cached usage JSON without probing providers first
  -h, --help    Print this help
";

#[derive(Debug, PartialEq, Eq)]
enum CliCommand {
    Usage { refresh: bool },
    Help,
}

pub fn handled_from_env() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() && !is_user_cli_entrypoint() {
        return false;
    }
    if args.first().is_some_and(|arg| arg.starts_with("-psn_")) {
        return false;
    }

    let command = if args.is_empty() {
        Ok(CliCommand::Usage { refresh: true })
    } else {
        parse_args(&args)
    };

    match command {
        Ok(CliCommand::Help) => {
            print!("{}", CLI_HELP);
            true
        }
        Ok(CliCommand::Usage { refresh }) => {
            match run_usage(refresh) {
                Ok(json) => println!("{}", json),
                Err(error) => {
                    eprintln!("openusage: {}", error);
                    std::process::exit(1);
                }
            }
            true
        }
        Err(error) => {
            eprintln!("openusage: {}", error);
            eprintln!("Run `openusage --help` for usage.");
            std::process::exit(2);
        }
    }
}

fn is_user_cli_entrypoint() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    cli_path().is_ok_and(|path| paths_resolve_to_same_file(&path, &exe))
}

fn parse_args(args: &[String]) -> Result<CliCommand, String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(CliCommand::Help);
    }

    let Some(command) = args.first().map(String::as_str) else {
        return Err("missing command".to_string());
    };

    match command {
        "usage" => {
            let refresh = parse_usage_flags(&args[1..])?;
            Ok(CliCommand::Usage { refresh })
        }
        "refresh" => {
            if let Some(flag) = args.get(1) {
                return Err(format!("unknown refresh option `{}`", flag));
            }
            Ok(CliCommand::Usage { refresh: true })
        }
        other => Err(format!("unknown command `{}`", other)),
    }
}

fn parse_usage_flags(flags: &[String]) -> Result<bool, String> {
    let mut refresh = true;
    for flag in flags {
        match flag.as_str() {
            "--no-refresh" => refresh = false,
            other => return Err(format!("unknown usage option `{}`", other)),
        }
    }
    Ok(refresh)
}

fn run_usage(refresh: bool) -> Result<String, String> {
    let app_data_dir = resolve_app_data_dir()?;
    let resource_dir = resolve_resource_dir()?;
    std::fs::create_dir_all(&app_data_dir)
        .map_err(|error| format!("failed to create app data dir: {}", error))?;

    let _proxy = config::get_resolved_proxy();
    let (_, plugins) = plugin_engine::initialize_plugins(&app_data_dir, &resource_dir);
    let known_plugin_ids: Vec<String> = plugins
        .iter()
        .map(|plugin| plugin.manifest.id.clone())
        .collect();
    local_http_api::init(&app_data_dir, known_plugin_ids.clone());

    if refresh {
        refresh_enabled_plugins(&app_data_dir, &plugins, &known_plugin_ids);
        local_http_api::flush_cache();
    }

    serde_json::to_string(&local_http_api::enabled_cached_snapshots())
        .map_err(|error| format!("failed to serialize usage JSON: {}", error))
}

fn refresh_enabled_plugins(
    app_data_dir: &PathBuf,
    plugins: &[plugin_engine::manifest::LoadedPlugin],
    known_plugin_ids: &[String],
) {
    let enabled_ids = local_http_api::enabled_plugin_ids_ordered(app_data_dir, known_plugin_ids);

    for plugin_id in enabled_ids {
        let Some(plugin) = plugins
            .iter()
            .find(|plugin| plugin.manifest.id == plugin_id)
            .cloned()
        else {
            continue;
        };

        let accounts = if plugin.manifest.id == "claude" || plugin.manifest.id == "codex" {
            accounts::load_credentials(app_data_dir, &plugin.manifest.id)
        } else {
            Vec::new()
        };

        if accounts.is_empty() {
            cache_probe_output(plugin_engine::runtime::run_probe(
                &plugin,
                app_data_dir,
                env!("CARGO_PKG_VERSION"),
            ));
            continue;
        }

        for account in accounts {
            cache_probe_output(plugin_engine::runtime::run_probe_for_account(
                &plugin,
                app_data_dir,
                env!("CARGO_PKG_VERSION"),
                Some(&account),
            ));
        }
    }
}

fn cache_probe_output(output: plugin_engine::runtime::PluginOutput) {
    let has_error = output.lines.iter().any(|line| {
        matches!(line, plugin_engine::runtime::MetricLine::Badge { label, .. } if label == "Error")
    });
    if !has_error {
        local_http_api::cache_successful_output(&output);
    }
}

fn resolve_app_data_dir() -> Result<PathBuf, String> {
    if let Ok(dir) = std::env::var("OPENUSAGE_APP_DATA_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }

    dirs::data_dir()
        .map(|dir| dir.join(APP_IDENTIFIER))
        .ok_or_else(|| "failed to resolve app data dir".to_string())
}

fn cli_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(".local/bin/openusage"))
        .ok_or_else(|| "home directory is unavailable".to_string())
}

fn resolve_resource_dir() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let cwd_candidates = [
        cwd.join("src-tauri").join("resources"),
        cwd.join("resources"),
        cwd.clone(),
    ];
    if let Some(candidate) = cwd_candidates
        .iter()
        .find(|candidate| has_bundled_plugins(candidate))
    {
        return Ok(candidate.clone());
    }

    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    for ancestor in exe.ancestors() {
        let candidates = [
            ancestor.to_path_buf(),
            ancestor.join("Resources"),
            ancestor.join("..").join("Resources"),
        ];
        if let Some(candidate) = candidates
            .iter()
            .find(|candidate| has_bundled_plugins(candidate))
        {
            return Ok(normalize_path(candidate));
        }
    }

    Err("failed to find bundled plugins".to_string())
}

fn has_bundled_plugins(path: &Path) -> bool {
    path.join("bundled_plugins").is_dir() || path.join("resources").join("bundled_plugins").is_dir()
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn paths_resolve_to_same_file(left: &Path, right: &Path) -> bool {
    normalize_path(left) == normalize_path(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parse_usage_defaults_to_refresh() {
        assert_eq!(
            parse_args(&args(&["usage"])).unwrap(),
            CliCommand::Usage { refresh: true }
        );
    }

    #[test]
    fn parse_usage_can_skip_refresh() {
        assert_eq!(
            parse_args(&args(&["usage", "--no-refresh"])).unwrap(),
            CliCommand::Usage { refresh: false }
        );
    }

    #[test]
    fn parse_refresh_alias() {
        assert_eq!(
            parse_args(&args(&["refresh"])).unwrap(),
            CliCommand::Usage { refresh: true }
        );
    }

    #[test]
    fn parse_help() {
        assert_eq!(parse_args(&args(&["--help"])).unwrap(), CliCommand::Help);
    }

    #[test]
    fn parse_unknown_command_fails() {
        assert!(parse_args(&args(&["nope"])).is_err());
    }

    #[test]
    fn symlinked_cli_entrypoint_matches_target_executable() {
        let root = std::env::temp_dir().join(format!(
            "openusage-cli-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin_dir = root.join(".local/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let target = root.join("OpenUsage.app/Contents/MacOS/openusage");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "").unwrap();
        let cli = bin_dir.join("openusage");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &cli).unwrap();

        #[cfg(windows)]
        std::fs::copy(&target, &cli).unwrap();

        assert!(paths_resolve_to_same_file(&cli, &target));

        let _ = std::fs::remove_dir_all(&root);
    }
}
