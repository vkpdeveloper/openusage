use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

const ACCOUNTS_FILE: &str = "accounts.json";
const VAULT_SERVICE: &str = "OpenUsage Provider Account";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAccount {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_home: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentLoginStatus {
    pub available: bool,
    pub is_new: bool,
}

#[derive(Debug, Clone)]
pub struct AccountCredential {
    pub account: ProviderAccount,
    pub order: usize,
    pub credential_json: String,
}

fn accounts_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(ACCOUNTS_FILE)
}

pub fn list(app_data_dir: &Path) -> Result<Vec<ProviderAccount>, String> {
    let path = accounts_path(app_data_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read saved accounts: {error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("Saved accounts are invalid: {error}"))
}

fn save_list(app_data_dir: &Path, accounts: &[ProviderAccount]) -> Result<(), String> {
    std::fs::create_dir_all(app_data_dir)
        .map_err(|error| format!("Failed to prepare account storage: {error}"))?;
    let path = accounts_path(app_data_dir);
    let temp_path = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(accounts)
        .map_err(|error| format!("Failed to encode saved accounts: {error}"))?;
    std::fs::write(&temp_path, text)
        .map_err(|error| format!("Failed to write saved accounts: {error}"))?;
    std::fs::rename(&temp_path, &path)
        .map_err(|error| format!("Failed to finish saving accounts: {error}"))
}

fn security_output(args: &[&str]) -> Result<String, String> {
    let output = Command::new("security")
        .args(args)
        .output()
        .map_err(|error| format!("Failed to access Keychain: {error}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        return Err(message
            .lines()
            .next()
            .unwrap_or("Keychain operation failed")
            .to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn read_keychain(service: &str, account: Option<&str>) -> Result<String, String> {
    let mut args = vec!["find-generic-password", "-s", service];
    if let Some(account) = account {
        args.extend(["-a", account]);
    }
    args.push("-w");
    security_output(&args)
}

fn write_keychain(account_id: &str, value: &str) -> Result<(), String> {
    security_output(&[
        "add-generic-password",
        "-U",
        "-s",
        VAULT_SERVICE,
        "-a",
        account_id,
        "-w",
        value,
    ])
    .map(|_| ())
}

fn delete_keychain(account_id: &str) -> Result<(), String> {
    security_output(&[
        "delete-generic-password",
        "-s",
        VAULT_SERVICE,
        "-a",
        account_id,
    ])
    .map(|_| ())
}

fn is_missing_keychain_item(error: &str) -> bool {
    error.contains("could not be found") || error.contains("SecKeychainSearchCopyNext")
}

fn current_macos_user() -> Option<String> {
    std::env::var("USER")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn read_first_file(paths: &[PathBuf]) -> Option<String> {
    paths.iter().find_map(|path| {
        std::fs::read_to_string(path)
            .ok()
            .filter(|text| serde_json::from_str::<serde_json::Value>(text).is_ok())
    })
}

fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn credential_fingerprint(provider_id: &str, credential_json: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(credential_json)
        .map_err(|_| "The current login contains invalid credential data.".to_string())?;

    let identity = if provider_id == "codex" {
        let tokens = value.get("tokens").and_then(|tokens| tokens.as_object());
        tokens
            .and_then(|tokens| tokens.get("account_id"))
            .and_then(|value| value.as_str())
            .map(|value| format!("account:{value}"))
            .or_else(|| {
                tokens
                    .and_then(|tokens| tokens.get("id_token"))
                    .and_then(|value| value.as_str())
                    .and_then(decode_jwt_payload)
                    .and_then(|payload| {
                        ["sub", "email"]
                            .into_iter()
                            .find_map(|key| payload.get(key).and_then(|value| value.as_str()))
                            .map(|value| format!("identity:{value}"))
                    })
            })
            .or_else(|| {
                tokens
                    .and_then(|tokens| tokens.get("refresh_token"))
                    .and_then(|value| value.as_str())
                    .map(|value| format!("refresh:{value}"))
            })
    } else {
        let oauth = value
            .get("claudeAiOauth")
            .and_then(|oauth| oauth.as_object());
        oauth
            .and_then(|oauth| oauth.get("accessToken"))
            .and_then(|value| value.as_str())
            .and_then(decode_jwt_payload)
            .and_then(|payload| {
                ["sub", "email", "user_id"]
                    .into_iter()
                    .find_map(|key| payload.get(key).and_then(|value| value.as_str()))
                    .map(|value| format!("identity:{value}"))
            })
            .or_else(|| {
                oauth
                    .and_then(|oauth| oauth.get("refreshToken"))
                    .and_then(|value| value.as_str())
                    .map(|value| format!("refresh:{value}"))
            })
    }
    .ok_or_else(|| "The current login has no stable account identity.".to_string())?;

    let digest = Sha256::digest(format!("{provider_id}:{identity}").as_bytes());
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn current_credential(provider_id: &str) -> Result<String, String> {
    match provider_id {
        "claude" => import_claude(),
        "codex" => import_codex(),
        _ => Err("Only Claude and Codex accounts can be saved.".to_string()),
    }
}

fn saved_fingerprints(app_data_dir: &Path, provider_id: &str) -> Vec<String> {
    load_credentials(app_data_dir, provider_id)
        .into_iter()
        .filter_map(|credential| {
            credential_fingerprint(provider_id, &credential.credential_json).ok()
        })
        .collect()
}

pub fn current_login_status(
    app_data_dir: &Path,
    provider_id: &str,
) -> Result<CurrentLoginStatus, String> {
    let credential_json = match current_credential(provider_id) {
        Ok(value) => value,
        Err(_) => {
            return Ok(CurrentLoginStatus {
                available: false,
                is_new: false,
            });
        }
    };
    let fingerprint = credential_fingerprint(provider_id, &credential_json)?;
    Ok(CurrentLoginStatus {
        available: true,
        is_new: !saved_fingerprints(app_data_dir, provider_id).contains(&fingerprint),
    })
}

fn import_claude() -> Result<String, String> {
    let keychain = current_macos_user()
        .and_then(|user| read_keychain("Claude Code-credentials", Some(&user)).ok())
        .or_else(|| read_keychain("Claude Code-credentials", None).ok());
    if let Some(value) = keychain {
        return Ok(value);
    }

    let home = dirs::home_dir().ok_or("Home directory is unavailable.")?;
    read_first_file(&[home.join(".claude/.credentials.json")]).ok_or_else(|| {
        "Claude is not logged in. Authenticate with Claude Code, then save the account again."
            .to_string()
    })
}

fn import_codex() -> Result<String, String> {
    let home = dirs::home_dir().ok_or("Home directory is unavailable.")?;
    let mut paths = Vec::new();
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        let trimmed = codex_home.trim();
        if !trimmed.is_empty() {
            paths.push(PathBuf::from(trimmed).join("auth.json"));
        }
    }
    paths.push(home.join(".config/codex/auth.json"));
    paths.push(home.join(".codex/auth.json"));

    read_first_file(&paths)
        .or_else(|| read_keychain("Codex Auth", None).ok())
        .ok_or_else(|| {
            "Codex is not logged in. Authenticate with Codex, then save the account again."
                .to_string()
        })
}

pub fn import_current(
    app_data_dir: &Path,
    provider_id: &str,
    name: &str,
) -> Result<ProviderAccount, String> {
    if !cfg!(target_os = "macos") {
        return Err("Saved accounts are currently supported on macOS only.".to_string());
    }
    if provider_id != "claude" && provider_id != "codex" {
        return Err("Only Claude and Codex accounts can be saved.".to_string());
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a profile name.".to_string());
    }
    if name.chars().count() > 60 {
        return Err("Profile names must be 60 characters or fewer.".to_string());
    }

    let credential_json = current_credential(provider_id)?;
    import_credential(app_data_dir, provider_id, name, &credential_json)
}

pub fn import_credential(
    app_data_dir: &Path,
    provider_id: &str,
    name: &str,
    credential_json: &str,
) -> Result<ProviderAccount, String> {
    import_credential_with_home(app_data_dir, provider_id, name, credential_json, None)
}

pub fn import_credential_with_home(
    app_data_dir: &Path,
    provider_id: &str,
    name: &str,
    credential_json: &str,
    credential_home: Option<String>,
) -> Result<ProviderAccount, String> {
    let name = name.trim();
    serde_json::from_str::<serde_json::Value>(&credential_json)
        .map_err(|_| "The current login contains invalid credential data.".to_string())?;
    let fingerprint = credential_fingerprint(provider_id, credential_json)?;
    if saved_fingerprints(app_data_dir, provider_id).contains(&fingerprint) {
        return Err("This account is already saved in OpenUsage.".to_string());
    }

    let account = ProviderAccount {
        id: Uuid::new_v4().to_string(),
        provider_id: provider_id.to_string(),
        name: name.to_string(),
        credential_home,
    };
    write_keychain(&account.id, credential_json)
        .map_err(|error| format!("Failed to save the account securely: {error}"))?;

    let mut accounts = list(app_data_dir)?;
    accounts.push(account.clone());
    if let Err(error) = save_list(app_data_dir, &accounts) {
        let _ = delete_keychain(&account.id);
        return Err(error);
    }
    Ok(account)
}

pub fn rename(app_data_dir: &Path, account_id: &str, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a profile name.".to_string());
    }
    let mut accounts = list(app_data_dir)?;
    let account = accounts
        .iter_mut()
        .find(|account| account.id == account_id)
        .ok_or("Saved account not found.")?;
    account.name = name.to_string();
    save_list(app_data_dir, &accounts)
}

pub fn remove(app_data_dir: &Path, account_id: &str) -> Result<(), String> {
    let mut accounts = list(app_data_dir)?;
    let credential_home = accounts
        .iter()
        .find(|account| account.id == account_id)
        .and_then(|account| account.credential_home.clone());
    let previous_len = accounts.len();
    accounts.retain(|account| account.id != account_id);
    if accounts.len() == previous_len {
        return Err("Saved account not found.".to_string());
    }
    if let Err(error) = delete_keychain(account_id) {
        if !is_missing_keychain_item(&error) {
            return Err(format!(
                "Failed to remove the account from Keychain: {error}"
            ));
        }
    }
    if let Some(path) = credential_home {
        let path = PathBuf::from(path);
        if path.exists() {
            let output = Command::new("/usr/bin/trash")
                .arg(&path)
                .output()
                .map_err(|error| {
                    format!("Failed to move the account directory to Trash: {error}")
                })?;
            if !output.status.success() {
                let message = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "Failed to move the account directory to Trash: {}",
                    message.lines().next().unwrap_or("Trash operation failed")
                ));
            }
        }
    }
    save_list(app_data_dir, &accounts)
}

pub fn load_credentials(app_data_dir: &Path, provider_id: &str) -> Vec<AccountCredential> {
    let Ok(accounts) = list(app_data_dir) else {
        log::error!("Failed to load saved account metadata");
        return Vec::new();
    };
    accounts
        .into_iter()
        .enumerate()
        .filter(|(_, account)| account.provider_id == provider_id)
        .filter_map(
            |(order, account)| match read_keychain(VAULT_SERVICE, Some(&account.id)) {
                Ok(credential_json) => Some(AccountCredential {
                    account,
                    order,
                    credential_json,
                }),
                Err(_) => {
                    log::error!(
                        "Saved {} account {} is missing its Keychain credential",
                        provider_id,
                        account.id
                    );
                    None
                }
            },
        )
        .collect()
}

pub fn update_credential(account_id: &str, credential_json: &str) -> Result<(), String> {
    serde_json::from_str::<serde_json::Value>(credential_json)
        .map_err(|_| "Refusing to save invalid account credentials.".to_string())?;
    write_keychain(account_id, credential_json)
        .map_err(|error| format!("Failed to update account credentials: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("openusage-accounts-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn list_returns_empty_when_account_index_is_missing() {
        let dir = temp_dir("missing");
        assert_eq!(list(&dir).unwrap(), Vec::<ProviderAccount>::new());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rename_updates_only_profile_metadata() {
        let dir = temp_dir("rename");
        save_list(
            &dir,
            &[ProviderAccount {
                id: "account-1".to_string(),
                provider_id: "claude".to_string(),
                name: "Old Name".to_string(),
                credential_home: None,
            }],
        )
        .unwrap();

        rename(&dir, "account-1", "Work").unwrap();

        assert_eq!(list(&dir).unwrap()[0].name, "Work");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_fingerprint_uses_account_id_across_token_rotation() {
        let first = r#"{"tokens":{"account_id":"account-1","refresh_token":"first"}}"#;
        let second = r#"{"tokens":{"account_id":"account-1","refresh_token":"second"}}"#;
        assert_eq!(
            credential_fingerprint("codex", first).unwrap(),
            credential_fingerprint("codex", second).unwrap()
        );
    }

    #[test]
    fn recognizes_missing_keychain_item_errors() {
        assert!(is_missing_keychain_item(
            "SecKeychainSearchCopyNext: The specified item could not be found in the keychain."
        ));
        assert!(!is_missing_keychain_item(
            "User interaction is not allowed."
        ));
    }

    #[test]
    fn claude_fingerprint_uses_jwt_subject_across_token_rotation() {
        let first = r#"{"claudeAiOauth":{"accessToken":"eyJhbGciOiJub25lIn0.eyJzdWIiOiJjbGF1ZGUtdXNlci0xIn0.signature","refreshToken":"first"}}"#;
        let second = r#"{"claudeAiOauth":{"accessToken":"eyJhbGciOiJub25lIn0.eyJzdWIiOiJjbGF1ZGUtdXNlci0xIn0.signature","refreshToken":"second"}}"#;
        assert_eq!(
            credential_fingerprint("claude", first).unwrap(),
            credential_fingerprint("claude", second).unwrap()
        );
    }
}
