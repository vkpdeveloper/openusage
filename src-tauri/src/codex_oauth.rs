use regex_lite::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthSession {
    pub id: String,
    pub status: String,
    pub authorization_url: Option<String>,
    pub error: Option<String>,
}

type Sessions = HashMap<String, Arc<Mutex<OAuthSession>>>;

fn sessions() -> &'static Mutex<Sessions> {
    static SESSIONS: OnceLock<Mutex<Sessions>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn codex_binary() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join("codex");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    let home = dirs::home_dir();
    let candidates = [
        Some(PathBuf::from("/opt/homebrew/bin/codex")),
        Some(PathBuf::from("/usr/local/bin/codex")),
        home.as_ref().map(|path| path.join(".bun/bin/codex")),
        home.as_ref().map(|path| path.join(".local/bin/codex")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "Codex CLI was not found. Install Codex before adding an account.".to_string()
        })
}

fn update_session<F>(session: &Arc<Mutex<OAuthSession>>, update: F)
where
    F: FnOnce(&mut OAuthSession),
{
    match session.lock() {
        Ok(mut locked) => update(&mut locked),
        Err(error) => log::error!("Failed to update Codex device auth session: {error}"),
    }
}

fn parse_prompt_line(session: &Arc<Mutex<OAuthSession>>, line: &str) {
    let ansi = Regex::new(r"\x1b\[[0-9;]*m").expect("valid ANSI regex");
    let cleaned = ansi.replace_all(line, "");
    let url = Regex::new(r"https://\S+/oauth/authorize\?\S+").expect("valid OAuth URL regex");

    if let Some(value) = url.find(&cleaned) {
        update_session(session, |state| {
            state.authorization_url = Some(value.as_str().to_string());
        });
    }
}

fn read_output<R: std::io::Read + Send + 'static>(
    output: R,
    session: Arc<Mutex<OAuthSession>>,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut collected = String::new();
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            parse_prompt_line(&session, &line);
            collected.push_str(&line);
            collected.push('\n');
        }
        collected
    })
}

fn run_session(
    session: Arc<Mutex<OAuthSession>>,
    app_data_dir: PathBuf,
    profile_name: String,
    codex_home: PathBuf,
    binary: PathBuf,
) {
    let mut child = match Command::new(binary)
        .args(["login", "-c", "cli_auth_credentials_store=\"file\""])
        .env("CODEX_HOME", &codex_home)
        .env("BROWSER", "/usr/bin/true")
        .env("BROWSERS", "/usr/bin/true")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            update_session(&session, |state| {
                state.status = "error".to_string();
                state.error = Some(format!("Failed to start Codex device auth: {error}"));
            });
            return;
        }
    };

    let stderr_thread = child
        .stderr
        .take()
        .map(|stderr| read_output(stderr, session.clone()));
    let stdout_thread = child
        .stdout
        .take()
        .map(|stdout| read_output(stdout, session.clone()));
    let child = Arc::new(Mutex::new(child));
    let timeout_child = child.clone();
    let timeout_session = session.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3 * 60));
        let should_kill = timeout_child
            .lock()
            .ok()
            .and_then(|mut process| process.try_wait().ok())
            .is_some_and(|status| status.is_none());
        if should_kill {
            if let Ok(mut process) = timeout_child.lock() {
                let _ = process.kill();
            }
            update_session(&timeout_session, |state| {
                state.status = "expired".to_string();
                state.error = None;
            });
        }
    });

    let status = child
        .lock()
        .map_err(|error| error.to_string())
        .and_then(|mut process| process.wait().map_err(|error| error.to_string()));
    let stderr = stderr_thread
        .and_then(|thread| thread.join().ok())
        .unwrap_or_default();
    let _stdout = stdout_thread
        .and_then(|thread| thread.join().ok())
        .unwrap_or_default();

    match status {
        Ok(status) if status.success() => {
            let auth_path = codex_home.join("auth.json");
            let credential_json = match std::fs::read_to_string(&auth_path) {
                Ok(value) => value,
                Err(error) => {
                    update_session(&session, |state| {
                        state.status = "error".to_string();
                        state.error = Some(format!(
                            "Codex completed login but did not save profile credentials: {error}"
                        ));
                    });
                    return;
                }
            };
            match crate::accounts::import_credential_with_home(
                &app_data_dir,
                "codex",
                &profile_name,
                &credential_json,
                Some(codex_home.to_string_lossy().to_string()),
            ) {
                Ok(_) => update_session(&session, |state| {
                    state.status = "complete".to_string();
                }),
                Err(error) => update_session(&session, |state| {
                    state.status = "error".to_string();
                    state.error = Some(error);
                }),
            }
        }
        Ok(_) | Err(_) => {
            if session
                .lock()
                .map(|state| state.status == "expired")
                .unwrap_or(false)
            {
                let _ = std::fs::remove_dir_all(&codex_home);
                return;
            }
            let message = stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("Codex device authentication failed.")
                .trim()
                .to_string();
            update_session(&session, |state| {
                state.status = "error".to_string();
                state.error = Some(message);
            });
        }
    }
}

pub fn start(app_data_dir: &Path, profile_name: &str) -> Result<OAuthSession, String> {
    let profile_name = profile_name.trim();
    if profile_name.is_empty() {
        return Err("Enter a profile name.".to_string());
    }
    if profile_name.chars().count() > 60 {
        return Err("Profile names must be 60 characters or fewer.".to_string());
    }

    let id = Uuid::new_v4().to_string();
    let home = dirs::home_dir().ok_or("Home directory is unavailable.")?;
    let codex_home = home.join(".openusage").join("codex").join(&id);
    std::fs::create_dir_all(&codex_home)
        .map_err(|error| format!("Failed to prepare Codex device auth: {error}"))?;

    let state = OAuthSession {
        id: id.clone(),
        status: "waiting".to_string(),
        authorization_url: None,
        error: None,
    };
    let shared = Arc::new(Mutex::new(state));
    sessions()
        .lock()
        .map_err(|error| error.to_string())?
        .insert(id, shared.clone());

    let binary = match codex_binary() {
        Ok(binary) => binary,
        Err(error) => {
            if let Ok(mut all) = sessions().lock() {
                all.remove(&shared_snapshot_from_arc(&shared)?.id);
            }
            return Err(error);
        }
    };
    let app_data_dir = app_data_dir.to_path_buf();
    let profile_name = profile_name.to_string();
    let worker_session = shared.clone();
    std::thread::spawn(move || {
        run_session(
            worker_session,
            app_data_dir,
            profile_name,
            codex_home,
            binary,
        );
    });

    for _ in 0..100 {
        let snapshot = shared_snapshot_from_arc(&shared)?;
        if snapshot.authorization_url.is_some() || snapshot.status == "error" {
            return Ok(snapshot);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(shared_snapshot_from_arc(&shared)?)
}

fn shared_snapshot_from_arc(session: &Arc<Mutex<OAuthSession>>) -> Result<OAuthSession, String> {
    session
        .lock()
        .map(|state| state.clone())
        .map_err(|error| error.to_string())
}

pub fn status(session_id: &str) -> Result<OAuthSession, String> {
    let session = sessions()
        .lock()
        .map_err(|error| error.to_string())?
        .get(session_id)
        .cloned()
        .ok_or_else(|| "Codex device auth session was not found.".to_string())?;
    shared_snapshot_from_arc(&session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_oauth_prompt() {
        let session = Arc::new(Mutex::new(OAuthSession {
            id: "test".to_string(),
            status: "waiting".to_string(),
            authorization_url: None,
            error: None,
        }));

        parse_prompt_line(
            &session,
            "\u{1b}[94mhttps://auth.openai.com/oauth/authorize?client_id=test&state=secret\u{1b}[0m",
        );

        let state = session.lock().unwrap();
        assert_eq!(
            state.authorization_url.as_deref(),
            Some("https://auth.openai.com/oauth/authorize?client_id=test&state=secret")
        );
    }
}
