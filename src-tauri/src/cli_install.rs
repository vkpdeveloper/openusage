use std::path::{Path, PathBuf};

const CLI_DIR_NAME: &str = ".local/bin";
const CLI_NAME: &str = "openusage";

pub fn install_for_current_user() {
    if let Err(error) = install_for_current_user_inner() {
        log::warn!("Failed to install OpenUsage CLI: {}", error);
    }
}

fn install_for_current_user_inner() -> Result<(), String> {
    let exe_path = std::env::current_exe()
        .map_err(|error| format!("failed to resolve current executable: {}", error))?;
    let cli_path = cli_path()?;

    if cli_path.exists() || cli_path.symlink_metadata().is_ok() {
        if points_to_current_exe(&cli_path, &exe_path) {
            return Ok(());
        }
        if is_symlink(&cli_path) {
            std::fs::remove_file(&cli_path)
                .map_err(|error| format!("failed to update CLI symlink: {}", error))?;
            create_command_link(&exe_path, &cli_path)?;
            log::info!("Updated OpenUsage CLI at {}", cli_path.display());
            return Ok(());
        }
        return Err(format!(
            "{} already exists and was not created by OpenUsage",
            cli_path.display()
        ));
    }

    let parent = cli_path
        .parent()
        .ok_or_else(|| "failed to resolve CLI install dir".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create CLI install dir: {}", error))?;
    create_command_link(&exe_path, &cli_path)?;
    log::info!("Installed OpenUsage CLI at {}", cli_path.display());
    Ok(())
}

fn cli_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(CLI_DIR_NAME).join(CLI_NAME))
        .ok_or_else(|| "home directory is unavailable".to_string())
}

fn points_to_current_exe(cli_path: &Path, exe_path: &Path) -> bool {
    let Ok(target) = std::fs::read_link(cli_path) else {
        return false;
    };
    normalize_path(&target) == normalize_path(exe_path)
}

fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

#[cfg(unix)]
fn create_command_link(exe_path: &Path, cli_path: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(exe_path, cli_path)
        .map_err(|error| format!("failed to create CLI symlink: {}", error))
}

#[cfg(windows)]
fn create_command_link(exe_path: &Path, cli_path: &Path) -> Result<(), String> {
    let cli_path = cli_path.with_extension("exe");
    std::fs::copy(exe_path, &cli_path)
        .map(|_| ())
        .map_err(|error| format!("failed to install CLI executable: {}", error))
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
