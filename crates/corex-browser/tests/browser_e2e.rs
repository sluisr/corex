use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use corex_browser::{Browser, Gesture};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};

const EXTENSION_ID: &str = "pgbencaoenigjeekljfindfeaolenaip";
const LOGO_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41,
    0x54, 0x78, 0xda, 0x63, 0xfc, 0xcf, 0xc0, 0x50, 0x0f, 0x00, 0x04, 0x85, 0x01, 0x80, 0x84, 0xa9, 0x8c, 0x21, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

type Saved = Arc<Mutex<HashMap<String, String>>>;

struct Site {
    port: u16,
    saved: Saved,
}

impl Site {
    fn url(&self, page: &str) -> String {
        format!("http://127.0.0.1:{}/{page}", self.port)
    }

    fn saved(&self, page: &str) -> Value {
        let body = self.saved.lock().unwrap().get(page).cloned().unwrap_or_default();
        serde_json::from_str(&body).unwrap_or(Value::Null)
    }
}

async fn serve(root: PathBuf) -> Site {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let saved: Saved = Arc::default();
    let shared = saved.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(answer(stream, root.clone(), shared.clone(), port));
        }
    });
    Site { port, saved }
}

async fn answer(mut stream: TcpStream, root: PathBuf, saved: Saved, port: u16) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let length = head
        .lines()
        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse().ok()))
        .unwrap_or(0usize);
    while buffer.len() < header_end + length {
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let mut request = head.lines().next().unwrap_or("").split_whitespace();
    let (method, target) = (request.next().unwrap_or(""), request.next().unwrap_or("/"));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    let (status, body) = if method == "POST" {
        let page = query.split('&').find_map(|pair| pair.strip_prefix("page=")).unwrap_or("unnamed");
        let body = String::from_utf8_lossy(&buffer[header_end..]).to_string();
        saved.lock().unwrap().insert(page.to_string(), body);
        ("200 OK", "ok".to_string())
    } else {
        match std::fs::read_to_string(root.join(path.trim_start_matches('/'))) {
            Ok(html) => ("200 OK", html.replace("__PORT__", &port.to_string())),
            Err(_) => ("404 Not Found", "not found".to_string()),
        }
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

fn find_browser() -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os("COREX_TEST_BROWSER") {
        return Some(PathBuf::from(custom));
    }
    let path = std::env::var_os("PATH")?;
    ["brave", "brave-browser", "google-chrome-stable", "google-chrome", "chromium", "chromium-browser"]
        .iter()
        .find_map(|name| std::env::split_paths(&path).map(|dir| dir.join(name)).find(|candidate| candidate.is_file()))
}

fn launch_browser(binary: &Path, workdir: &Path, real_home: Option<std::ffi::OsString>) -> Child {
    let profile = workdir.join("profile");
    let hosts = profile.join("NativeMessagingHosts");
    std::fs::create_dir_all(&hosts).unwrap();
    let manifest = json!({
        "name": "com.corex.bridge",
        "description": "Corex end-to-end test host",
        "path": env!("CARGO_BIN_EXE_corex-browser-host"),
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    std::fs::write(hosts.join("com.corex.bridge.json"), manifest.to_string()).unwrap();

    let mut command = Command::new(binary);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--load-extension={}", Path::new(env!("CARGO_MANIFEST_DIR")).join("extension").display()))
        .args([
            "--headless=new",
            "--window-size=1280,900",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-features=DisableLoadExtensionCommandLineSwitch",
            "about:blank",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(home) = real_home {
        command.env("HOME", home);
    }
    command.spawn().expect("the browser starts")
}

fn find(snapshot: &str, needles: &[&str]) -> String {
    snapshot
        .lines()
        .filter(|line| line.trim_start().starts_with("- ") && needles.iter().all(|needle| line.contains(needle)))
        .find_map(|line| line.split("[ref=").nth(1))
        .and_then(|rest| rest.split(']').next())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no ref for {needles:?} in:\n{snapshot}"))
}

struct Page {
    browser: Browser,
    snapshot: String,
}

impl Page {
    async fn open(&mut self, url: &str) {
        self.browser.navigate(url).await.unwrap();
        self.refresh().await;
    }

    async fn refresh(&mut self) {
        self.snapshot = self.browser.snapshot().await.unwrap();
    }

    async fn click(&mut self, needles: &[&str]) {
        let reference = find(&self.snapshot, needles);
        self.browser.click(&reference).await.unwrap_or_else(|e| panic!("click {needles:?}: {e}"));
        self.refresh().await;
    }

    async fn gesture(&mut self, needles: &[&str], gesture: Gesture) {
        let reference = find(&self.snapshot, needles);
        self.browser.pointer(&reference, gesture).await.unwrap();
        self.refresh().await;
    }

    async fn type_into(&mut self, needles: &[&str], text: &str, submit: bool) {
        let reference = find(&self.snapshot, needles);
        self.browser.type_text(&reference, text, true, submit).await.unwrap();
        self.refresh().await;
    }

    async fn select(&mut self, needles: &[&str], value: &str) {
        let reference = find(&self.snapshot, needles);
        self.browser.select_option(&reference, value).await.unwrap();
        self.refresh().await;
    }

    async fn choose(&mut self, needles: &[&str], value: &str) {
        let reference = find(&self.snapshot, needles);
        self.browser.choose(&reference, value).await.unwrap();
        self.refresh().await;
    }

    async fn upload(&mut self, needles: &[&str], path: &Path) {
        let reference = find(&self.snapshot, needles);
        self.browser.upload(&reference, &[path.display().to_string()]).await.unwrap();
        self.refresh().await;
    }

    async fn key(&mut self, key: &str) {
        self.browser.press_key(key).await.unwrap();
    }

    async fn text(&self) -> String {
        self.browser.page_text().await.unwrap()
    }
}

async fn settings_panel(page: &mut Page, site: &Site, logo: &Path) {
    page.open(&site.url("settings.html")).await;
    page.choose(&["combobox", "English (US)"], "Spanish (Mexico)").await;
    page.select(&["combobox", "Time zone"], "America/Mexico_City").await;
    page.click(&["switch", "Dark mode"]).await;
    page.click(&["tab", "Notifications"]).await;
    page.click(&["switch", "Email"]).await;
    page.click(&["switch", "Push"]).await;
    page.click(&["slider"]).await;
    for _ in 0..4 {
        page.key("ArrowRight").await;
    }
    page.type_into(&["textbox", "Tags"], "ops", true).await;
    page.type_into(&["textbox", "Tags"], "urgent", true).await;
    page.click(&["tab", "Billing"]).await;
    page.click(&["radio", "Pro"]).await;
    page.click(&["radio", "Yearly"]).await;
    page.click(&["button", "Pick a date"]).await;
    page.click(&["button", "Next month"]).await;
    page.click(&["December 15, 2026"]).await;
    page.upload(&["button", "Company logo"], logo).await;
    page.click(&["tab", "Security"]).await;
    page.click(&["Advanced security"]).await;
    page.click(&["switch", "Two-factor"]).await;
    page.click(&["button", "Save changes"]).await;

    let saved = site.saved("settings");
    assert_eq!(
        saved,
        json!({
            "lang": "Spanish (Mexico)", "tz": "America/Mexico_City", "dark": true, "email": true, "push": false,
            "volume": 70, "tags": ["ops", "urgent"], "plan": "Pro", "cycle": "yearly", "start": "2026-12-15",
            "logo": "logo.png", "twofa": true, "keysDeleted": false
        })
    );
}

async fn wizard(page: &mut Page, site: &Site) {
    page.open(&site.url("wizard.html")).await;
    page.click(&["button", "Next"]).await;
    assert!(page.text().await.contains("Fill in company"), "validation must block an empty first step");
    page.type_into(&["textbox", "Company name"], "Acme Ltd", false).await;
    page.type_into(&["textbox", "Contact email"], "ops@acme.test", false).await;
    page.select(&["combobox", "Team size"], "51-200").await;
    page.click(&["button", "Next"]).await;
    page.click(&["radio", "South America"]).await;
    page.click(&["checkbox", "Managed database"]).await;
    page.click(&["checkbox", "Daily backups"]).await;
    page.type_into(&["textbox", "Notes"], "Migration planned for January", false).await;
    page.click(&["button", "Next"]).await;
    page.click(&["checkbox", "terms"]).await;
    page.click(&["button", "Create project"]).await;

    assert_eq!(
        site.saved("wizard"),
        json!({
            "region": "sa-east", "company": "Acme Ltd", "email": "ops@acme.test", "size": "51-200",
            "db": true, "cdn": false, "backups": true, "notes": "Migration planned for January"
        })
    );
}

async fn covered_clicks(page: &mut Page, site: &Site) {
    page.open(&site.url("overlay.html")).await;
    let reference = find(&page.snapshot, &["button", "Save order"]);
    let error = page.browser.click(&reference).await.expect_err("a covered button must not be clicked");
    assert!(error.to_string().contains("covered by"), "{error}");
    assert!(site.saved("overlay").is_null());
    page.click(&["button", "Accept cookies"]).await;
    page.click(&["button", "Save order"]).await;
    assert!(!site.saved.lock().unwrap().get("overlay").cloned().unwrap_or_default().is_empty());

    page.open(&site.url("sticky.html")).await;
    page.click(&["button", "Deep button"]).await;
    assert!(!site.saved.lock().unwrap().get("sticky").cloned().unwrap_or_default().is_empty());
}

async fn gestures(page: &mut Page, site: &Site, logo: &Path) {
    page.open(&site.url("gestures.html")).await;
    page.choose(&["button", "Country"], "Spain").await;
    assert!(page.text().await.contains("Picked: Spain"));
    let reference = find(&page.snapshot, &["button", "Country"]);
    let error = page.browser.choose(&reference, "Narnia").await.expect_err("a missing option is an error");
    assert!(error.to_string().contains("Options visible"), "{error}");
    page.refresh().await;

    page.gesture(&["button", "Menu"], Gesture::Hover).await;
    assert!(page.snapshot.contains("Hover option"));
    page.gesture(&["button", "Double"], Gesture::Double).await;
    page.gesture(&["button", "Right"], Gesture::Right).await;
    page.upload(&["button", "attachment"], logo).await;
    let text = page.text().await;
    for expected in ["double-ok", "right-ok", "file:logo.png"] {
        assert!(text.contains(expected), "missing {expected} in:\n{text}");
    }

    let alert = find(&page.snapshot, &["button", "Alert"]);
    page.browser.click(&alert).await.expect("a click that opens an alert returns");
    assert!(page.browser.snapshot().await.is_err());
    assert_eq!(page.browser.open_dialog().as_deref(), Some("alert: hello"));
    page.browser.dialog(true, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(page.browser.open_dialog().is_none());
}

async fn workspace(page: &mut Page, site: &Site) {
    page.open(&site.url("workspace.html")).await;
    assert!(page.snapshot.contains("[invalid=true] description=\"Enter a valid address\""), "{}", page.snapshot);
    let filtered = page.browser.snapshot_matching(Some("Member 40")).await.unwrap();
    assert!(filtered.contains("option \"Member 40\" [ref="), "{filtered}");
    page.refresh().await;

    let members = find(&page.snapshot, &["listbox", "Members"]);
    page.browser.scroll("down", Some(&members)).await.unwrap();
    assert!(!page.text().await.contains("scrolled: 0"));

    page.click(&["textbox", "Name"]).await;
    page.key("Control+a").await;
    page.key("Backspace").await;
    let text = page.text().await;
    assert!(text.contains("Name is:") && !text.contains("old value"), "{text}");

    let started = std::time::Instant::now();
    assert!(page.browser.wait_for_text("Search or start", false, Duration::from_secs(5)).await.unwrap());
    assert!(started.elapsed() < Duration::from_secs(1), "placeholder found after {:?}", started.elapsed());

    let save = find(&page.snapshot, &["button", "Save"]);
    page.browser.click(&save).await.unwrap();
    let started = std::time::Instant::now();
    page.browser.click(&save).await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(1200), "a click on a busy page took {:?}", started.elapsed());
    page.refresh().await;
    assert!(page.browser.wait_for_text("Saved changes", false, Duration::from_secs(5)).await.unwrap());
    assert!(page.browser.wait_for_text("Saving", true, Duration::from_secs(1)).await.unwrap());

    page.click(&["button", "Open help"]).await;
    let mut note = None;
    for _ in 0..20 {
        note = page.browser.sync_tabs().await.unwrap();
        if note.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let note = note.expect("the tab opened by the page is followed");
    assert!(note.contains("Help center"), "{note}");
    page.refresh().await;
    page.click(&["button", "Mark as read"]).await;
    let tabs = page.browser.list_tabs().await;
    assert_eq!(tabs.len(), 2);
    let help = tabs.iter().find(|tab| tab.active).expect("an active tab");
    assert_eq!(help.title, "Help read");
    page.browser.close_tab(help.id).await.unwrap();
    assert!(page.browser.page_info().await.contains("Workspace settings"));
    assert_eq!(page.browser.list_tabs().await.len(), 1);
}

async fn frames(page: &mut Page, site: &Site) {
    for (url, message) in [("embedded.html", "same origin"), ("embedded-x.html", "cross origin")] {
        page.open(&site.url(url)).await;
        page.select(&["combobox", "Topic"], "Technical support").await;
        page.type_into(&["textbox", "Message"], message, false).await;
        page.click(&["button", "Send message"]).await;
        assert_eq!(site.saved("iframe"), json!({ "topic": "Technical support", "msg": message }), "{url}");
    }
}

#[tokio::test]
#[ignore = "needs a local Chromium-based browser; run with --ignored"]
async fn drives_real_pages_through_the_extension() {
    let Some(binary) = find_browser() else {
        println!("no Chromium-based browser found, set COREX_TEST_BROWSER to run this test");
        return;
    };
    let workdir = std::env::temp_dir().join(format!("corex-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let logo = workdir.join("logo.png");
    std::fs::write(&logo, LOGO_PNG).unwrap();

    let real_home = std::env::var_os("HOME");
    std::env::set_var("HOME", workdir.join("home"));
    std::env::set_var("XDG_DATA_HOME", workdir.join("data"));

    let site = serve(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")).await;
    let _browser = launch_browser(&binary, &workdir, real_home);
    let browser = Browser::start().await.expect("the extension connects to Corex");
    let mut page = Page { browser, snapshot: String::new() };

    settings_panel(&mut page, &site, &logo).await;
    wizard(&mut page, &site).await;
    covered_clicks(&mut page, &site).await;
    gestures(&mut page, &site, &logo).await;
    workspace(&mut page, &site).await;
    frames(&mut page, &site).await;

    drop(page);
    let _ = std::fs::remove_dir_all(&workdir);
}
