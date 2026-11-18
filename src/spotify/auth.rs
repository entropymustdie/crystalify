use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use librespot::core::authentication::Credentials;
use librespot::core::cache::Cache;
use librespot::core::config::SessionConfig;
use librespot::core::session::Session;
use librespot::discovery::DeviceType;
use librespot::oauth::OAuthClientBuilder;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const SPOTIFY_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
const SPOTIFY_REDIRECT_URI: &str = "http://127.0.0.1:8898/login";
const SPOTIFY_SCOPES: &[&str] = &[
    "streaming",
    "user-read-private",
    "playlist-read-private",
    "playlist-read-collaborative",
];

const OAUTH_SUCCESS_HTML: &str = r#"<!doctype html>
<html>
<head>
    <meta charset="utf-8">
    <title>Crystalify — Authenticated</title>
    <style>
        body { font-family: system-ui, sans-serif; background: #121212; color: #1ed760; display: flex; align-items: center; justify-content: center; height: 100vh; margin: 0; }
        .card { background: #181818; padding: 2rem 3rem; border-radius: 12px; text-align: center; box-shadow: 0 8px 24px rgba(0,0,0,0.5); }
        p { color: #b3b3b3; }
    </style>
</head>
<body>
    <div class="card">
        <h1>Crystalify Authenticated!</h1>
        <p>You can close this tab and return to your terminal.</p>
    </div>
</body>
</html>"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    OAuth,
    Discovery,
    Password,
}

pub fn parse_auth_choice(input: &str) -> Result<AuthMethod, &'static str> {
    match input.trim() {
        "" | "1" | "oauth" | "google" | "browser" => Ok(AuthMethod::OAuth),
        "2" | "discovery" | "connect" => Ok(AuthMethod::Discovery),
        "3" | "password" | "login" => Ok(AuthMethod::Password),
        _ => Err("Invalid choice. Please enter 1, 2, or 3."),
    }
}

pub fn zeroize_string(s: &mut String) {
    unsafe {
        s.as_bytes_mut().fill(0);
    }
    s.clear();
}

pub fn resolve_cache_dir_for_platform(
    xdg_cache_home: Option<&str>,
    local_app_data: Option<&str>,
    user_profile: Option<&str>,
    home: Option<&str>,
    is_windows: bool,
) -> Option<PathBuf> {
    if let Some(xdg) = xdg_cache_home.filter(|s| !s.trim().is_empty()) {
        return Some(PathBuf::from(xdg).join("crystalify"));
    }
    if is_windows {
        if let Some(local) = local_app_data.filter(|s| !s.trim().is_empty()) {
            return Some(PathBuf::from(local).join("crystalify"));
        }
        if let Some(profile) = user_profile.filter(|s| !s.trim().is_empty()) {
            return Some(PathBuf::from(profile).join(".cache").join("crystalify"));
        }
    }
    if let Some(h) = home.filter(|s| !s.trim().is_empty()) {
        return Some(PathBuf::from(h).join(".cache").join("crystalify"));
    }
    None
}

#[cfg(test)]
pub fn resolve_cache_dir_from(xdg_cache_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    resolve_cache_dir_for_platform(xdg_cache_home, None, None, home, false)
}

pub fn get_cache_dir() -> Option<PathBuf> {
    let xdg = env::var("XDG_CACHE_HOME").ok();
    let local_app_data = env::var("LOCALAPPDATA").ok();
    let user_profile = env::var("USERPROFILE").ok();
    let home = env::var("HOME").ok();
    resolve_cache_dir_for_platform(
        xdg.as_deref(),
        local_app_data.as_deref(),
        user_profile.as_deref(),
        home.as_deref(),
        cfg!(windows),
    )
}

pub fn init_cache_at(dir: &Path) -> Result<Cache, Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o700);
        fs::set_permissions(dir, perms)?;
    }
    let cache = Cache::new(Some(dir), Some(dir), None, None)?;
    Ok(cache)
}

pub fn init_cache() -> Option<Cache> {
    let dir = get_cache_dir()?;
    init_cache_at(&dir).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordKeyAction {
    None,
    EchoStar,
    EraseStar,
    Submit,
    Cancel,
}

pub fn handle_password_key(key: KeyEvent, buffer: &mut String) -> PasswordKeyAction {
    if key.kind != KeyEventKind::Press {
        return PasswordKeyAction::None;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
    {
        zeroize_string(buffer);
        return PasswordKeyAction::Cancel;
    }

    match key.code {
        KeyCode::Esc => {
            zeroize_string(buffer);
            PasswordKeyAction::Cancel
        }
        KeyCode::Enter => PasswordKeyAction::Submit,
        KeyCode::Backspace => {
            if buffer.pop().is_some() {
                PasswordKeyAction::EraseStar
            } else {
                PasswordKeyAction::None
            }
        }
        KeyCode::Char(c) => {
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
            {
                buffer.push(c);
                PasswordKeyAction::EchoStar
            } else {
                PasswordKeyAction::None
            }
        }
        _ => PasswordKeyAction::None,
    }
}

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

pub fn read_password_masked(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    print!("{prompt}");
    io::stdout().flush()?;

    let _guard = RawModeGuard::new()?;
    let mut password = String::new();

    loop {
        let ev = match event::read() {
            Ok(ev) => ev,
            Err(e) => {
                zeroize_string(&mut password);
                return Err(e.into());
            }
        };

        if let Event::Key(key) = ev {
            match handle_password_key(key, &mut password) {
                PasswordKeyAction::None => {}
                PasswordKeyAction::EchoStar => {
                    print!("*");
                    io::stdout().flush()?;
                }
                PasswordKeyAction::EraseStar => {
                    print!("\x08 \x08");
                    io::stdout().flush()?;
                }
                PasswordKeyAction::Submit => {
                    print!("\r\n");
                    io::stdout().flush()?;
                    return Ok(password);
                }
                PasswordKeyAction::Cancel => {
                    print!("\r\n");
                    io::stdout().flush()?;
                    return Err("Authentication cancelled by user".into());
                }
            }
        }
    }
}

async fn obtain_oauth_credentials() -> Result<Credentials, Box<dyn std::error::Error>> {
    println!("Starting Spotify OAuth 2.0 login (supports Google, Apple, Facebook, or email)...");
    println!("Your browser will open automatically, or you can copy the URL shown below.");

    let client = OAuthClientBuilder::new(
        SPOTIFY_CLIENT_ID,
        SPOTIFY_REDIRECT_URI,
        SPOTIFY_SCOPES.to_vec(),
    )
    .open_in_browser()
    .with_custom_message(OAUTH_SUCCESS_HTML)
    .build()?;

    let mut token = tokio::task::spawn_blocking(move || client.get_access_token()).await??;
    let credentials = Credentials::with_access_token(&token.access_token);
    zeroize_string(&mut token.access_token);
    zeroize_string(&mut token.refresh_token);

    Ok(credentials)
}

async fn obtain_discovery_credentials() -> Result<Credentials, Box<dyn std::error::Error>> {
    println!("Launching Spotify Connect discovery as 'crystalify'...");
    println!("Open Spotify on your phone/PC and select 'crystalify' in available devices.");
    let mut discovery = librespot::discovery::Discovery::builder(
        SessionConfig::default().device_id,
        SPOTIFY_CLIENT_ID.to_string(),
    )
    .name("crystalify")
    .device_type(DeviceType::Computer)
    .launch()?;

    match discovery.next().await {
        Some(creds) => Ok(creds),
        None => Err("Discovery closed without credentials".into()),
    }
}

fn obtain_password_credentials() -> Result<Credentials, Box<dyn std::error::Error>> {
    print!("Enter Spotify Username: ");
    io::stdout().flush()?;
    let mut user_input = String::new();
    io::stdin().read_line(&mut user_input)?;
    let username = user_input.trim().to_string();
    if username.is_empty() {
        return Err("Username cannot be empty".into());
    }

    let mut pass_input = read_password_masked("Enter Spotify Password: ")?;
    let trimmed_pass = pass_input.trim().to_string();
    zeroize_string(&mut pass_input);

    if trimmed_pass.is_empty() {
        return Err("Password cannot be empty".into());
    }

    let mut pass_holder = trimmed_pass;
    let creds = Credentials::with_password(username, &pass_holder);
    zeroize_string(&mut pass_holder);

    Ok(creds)
}

fn prompt_auth_method() -> Result<AuthMethod, Box<dyn std::error::Error>> {
    println!("\nSelect Spotify authentication method:");
    println!("  [1] Browser / Google login (Spotify OAuth 2.0) — recommended (default)");
    println!("  [2] Spotify Connect Discovery (select 'crystalify' in Spotify app)");
    println!("  [3] Username & Password (masked input)");

    loop {
        print!("Choice [1]: ");
        io::stdout().flush()?;
        let mut choice = String::new();
        if io::stdin().read_line(&mut choice)? == 0 {
            return Err("Unexpected EOF while reading authentication choice".into());
        }
        match parse_auth_choice(&choice) {
            Ok(method) => return Ok(method),
            Err(msg) => println!("{msg}"),
        }
    }
}

async fn connect_session(
    credentials: Credentials,
    cache: Option<Cache>,
) -> Result<Session, Box<dyn std::error::Error>> {
    let session_config = SessionConfig::default();
    let session = Session::new(session_config, cache);
    session.connect(credentials, true).await?;
    Ok(session)
}

pub async fn authenticate() -> Result<Session, Box<dyn std::error::Error>> {
    let cache = init_cache();

    let mut env_token = env::var("SPOTIFY_ACCESS_TOKEN").unwrap_or_default();
    let env_username = env::var("SPOTIFY_USERNAME").unwrap_or_default();
    let mut env_password = env::var("SPOTIFY_PASSWORD").unwrap_or_default();

    let has_env_token = !env_token.trim().is_empty();
    let has_env_user_pass = !env_username.trim().is_empty() && !env_password.trim().is_empty();

    if !has_env_token
        && !has_env_user_pass
        && let Some(ref c) = cache
        && let Some(cached_creds) = c.credentials()
    {
        println!("Found cached Spotify credentials, connecting...");
        match connect_session(cached_creds, cache.clone()).await {
            Ok(session) => {
                println!("Logged in as: {}", session.username());
                zeroize_string(&mut env_token);
                zeroize_string(&mut env_password);
                return Ok(session);
            }
            Err(e) => {
                eprintln!(
                    "Cached credentials expired or failed ({e}). Falling back to interactive login..."
                );
            }
        }
    }

    let credentials = if has_env_token {
        let creds = Credentials::with_access_token(env_token.trim());
        zeroize_string(&mut env_token);
        zeroize_string(&mut env_password);
        creds
    } else if has_env_user_pass {
        let creds = Credentials::with_password(env_username.trim(), env_password.trim());
        zeroize_string(&mut env_password);
        creds
    } else {
        zeroize_string(&mut env_token);
        zeroize_string(&mut env_password);
        match prompt_auth_method()? {
            AuthMethod::OAuth => obtain_oauth_credentials().await?,
            AuthMethod::Discovery => obtain_discovery_credentials().await?,
            AuthMethod::Password => obtain_password_credentials()?,
        }
    };

    println!("Connecting to Spotify...");
    let session = connect_session(credentials, cache).await?;
    println!("Logged in as: {}", session.username());

    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn test_zeroize_string_clears_and_overwrites_memory() {
        let mut secret = String::from("SuperSecretPassword123!@#");
        let ptr = secret.as_ptr();
        let len = secret.len();
        assert!(len > 0);

        zeroize_string(&mut secret);
        assert!(secret.is_empty());
        assert_eq!(secret.len(), 0);

        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        assert!(slice.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_parse_auth_choice() {
        assert_eq!(parse_auth_choice(""), Ok(AuthMethod::OAuth));
        assert_eq!(parse_auth_choice("   \n "), Ok(AuthMethod::OAuth));
        assert_eq!(parse_auth_choice("1"), Ok(AuthMethod::OAuth));
        assert_eq!(parse_auth_choice(" 1 "), Ok(AuthMethod::OAuth));
        assert_eq!(parse_auth_choice("google"), Ok(AuthMethod::OAuth));
        assert_eq!(parse_auth_choice("oauth"), Ok(AuthMethod::OAuth));

        assert_eq!(parse_auth_choice("2"), Ok(AuthMethod::Discovery));
        assert_eq!(parse_auth_choice("discovery"), Ok(AuthMethod::Discovery));

        assert_eq!(parse_auth_choice("3"), Ok(AuthMethod::Password));
        assert_eq!(parse_auth_choice("password"), Ok(AuthMethod::Password));

        assert!(parse_auth_choice("4").is_err());
        assert!(parse_auth_choice("invalid").is_err());
    }

    #[test]
    fn test_handle_password_key_typing_and_submit() {
        let mut buf = String::new();

        assert_eq!(
            handle_password_key(
                make_key(KeyCode::Char('s'), KeyModifiers::NONE, KeyEventKind::Press),
                &mut buf
            ),
            PasswordKeyAction::EchoStar
        );
        assert_eq!(
            handle_password_key(
                make_key(KeyCode::Char('P'), KeyModifiers::SHIFT, KeyEventKind::Press),
                &mut buf
            ),
            PasswordKeyAction::EchoStar
        );
        assert_eq!(
            handle_password_key(
                make_key(KeyCode::Char('ї'), KeyModifiers::NONE, KeyEventKind::Press),
                &mut buf
            ),
            PasswordKeyAction::EchoStar
        );
        assert_eq!(buf, "sPї");

        assert_eq!(
            handle_password_key(
                make_key(
                    KeyCode::Char('x'),
                    KeyModifiers::NONE,
                    KeyEventKind::Release
                ),
                &mut buf
            ),
            PasswordKeyAction::None
        );
        assert_eq!(buf, "sPї");

        assert_eq!(
            handle_password_key(
                make_key(KeyCode::Backspace, KeyModifiers::NONE, KeyEventKind::Press),
                &mut buf
            ),
            PasswordKeyAction::EraseStar
        );
        assert_eq!(buf, "sP");

        assert_eq!(
            handle_password_key(
                make_key(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Press),
                &mut buf
            ),
            PasswordKeyAction::Submit
        );
        assert_eq!(buf, "sP");
    }

    #[test]
    fn test_handle_password_key_backspace_on_empty() {
        let mut buf = String::new();
        assert_eq!(
            handle_password_key(
                make_key(KeyCode::Backspace, KeyModifiers::NONE, KeyEventKind::Press),
                &mut buf
            ),
            PasswordKeyAction::None
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn test_handle_password_key_cancel_zeroizes_buffer() {
        let mut buf = String::from("my_secret");
        let ptr = buf.as_ptr();
        let len = buf.len();

        assert_eq!(
            handle_password_key(
                make_key(
                    KeyCode::Char('c'),
                    KeyModifiers::CONTROL,
                    KeyEventKind::Press
                ),
                &mut buf
            ),
            PasswordKeyAction::Cancel
        );
        assert!(buf.is_empty());
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        assert!(slice.iter().all(|&b| b == 0));

        let mut buf2 = String::from("another_secret");
        assert_eq!(
            handle_password_key(
                make_key(KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Press),
                &mut buf2
            ),
            PasswordKeyAction::Cancel
        );
        assert!(buf2.is_empty());
    }

    #[test]
    fn test_resolve_cache_dir_from() {
        assert_eq!(
            resolve_cache_dir_from(Some("/custom/xdg"), Some("/home/user")),
            Some(PathBuf::from("/custom/xdg/crystalify"))
        );
        assert_eq!(
            resolve_cache_dir_from(Some(""), Some("/home/user")),
            Some(PathBuf::from("/home/user/.cache/crystalify"))
        );
        assert_eq!(
            resolve_cache_dir_from(None, Some("/home/user")),
            Some(PathBuf::from("/home/user/.cache/crystalify"))
        );
        assert_eq!(resolve_cache_dir_from(None, None), None);
    }

    #[test]
    fn test_resolve_cache_dir_for_platform_windows_and_unix() {
        assert_eq!(
            resolve_cache_dir_for_platform(
                Some("D:\\xdg"),
                Some("C:\\Users\\user\\AppData\\Local"),
                Some("C:\\Users\\user"),
                Some("/home/user"),
                true
            ),
            Some(PathBuf::from("D:\\xdg").join("crystalify"))
        );
        assert_eq!(
            resolve_cache_dir_for_platform(
                Some("   "),
                Some("C:\\Users\\user\\AppData\\Local"),
                Some("C:\\Users\\user"),
                None,
                true
            ),
            Some(PathBuf::from("C:\\Users\\user\\AppData\\Local").join("crystalify"))
        );
        assert_eq!(
            resolve_cache_dir_for_platform(
                None,
                Some(""),
                Some("C:\\Users\\user"),
                None,
                true
            ),
            Some(
                PathBuf::from("C:\\Users\\user")
                    .join(".cache")
                    .join("crystalify")
            )
        );
        assert_eq!(
            resolve_cache_dir_for_platform(
                None,
                Some("C:\\Users\\user\\AppData\\Local"),
                Some("C:\\Users\\user"),
                Some("/home/user"),
                false
            ),
            Some(PathBuf::from("/home/user").join(".cache").join("crystalify"))
        );
        assert_eq!(
            resolve_cache_dir_for_platform(None, None, None, None, true),
            None
        );
    }

    #[test]
    fn test_init_cache_at_creates_dir_with_0700_permissions() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = env::temp_dir().join(format!("crystalify_test_cache_{nanos}"));

        let cache = init_cache_at(&temp_dir).expect("Failed to initialize cache");
        assert!(temp_dir.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&temp_dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }

        let creds = Credentials::with_access_token("test_oauth_token_blob");
        cache.save_credentials(&creds);
        let loaded = cache.credentials().expect("Should load saved credentials");
        assert_eq!(loaded, creds);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
