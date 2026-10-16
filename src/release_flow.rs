//! Release flow — Rust replacement for the Velopack usage in App.xaml.cs
//! (`UpdateManager(GithubSource(...))`, `CheckForUpdatesAsync`, balloon tip,
//! apply-on-exit + restart). Observable behavior preserved: startup check,
//! tray balloon on download, Restart-with-version label, apply-then-restart.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use thiserror::Error;

/// Failure classes of the update check. The source swallows every one of them
/// (catch { /* Update check is best-effort */ }); the port returns them so the
/// caller can log what happened.
#[derive(Debug, Error)]
pub enum ReleaseError {
    #[error("network: {0}")]
    Network(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("io: {0}")]
    Io(String),
}

/// What the app needs to finish an update: the version, the installer asset's URL
/// and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// Version from the GitHub tag (e.g. "0.4.9" from "v0.4.9").
    pub version: String,
    pub asset_url: String,
    pub asset_name: String,
}

/// Port of UpdateManager(new GithubSource(...)) for the observable half of the
/// source's Velopack usage: check, download, apply-on-exit, then restart.
pub struct ReleaseManager {
    /// owner/name, or an API base URL (used by the wire tests).
    repo: String,
}

impl ReleaseManager {
    pub fn new(repo: &str) -> Self {
        Self {
            repo: repo.to_string(),
        }
    }

    /// C# `CheckForUpdatesAsync` — Some(info) when the latest release is newer than
    /// `current_version`. Uses the GitHub API latest-release endpoint with a UA header.
    pub async fn check_for_updates(
        &self,
        current_version: &str,
    ) -> Result<Option<ReleaseInfo>, ReleaseError> {
        crate::llm::ensure_tls_provider();

        // GitHub rejects any request without a User-Agent; the Accept header pins the
        // documented `+json` media type.
        let response = reqwest::Client::new()
            .get(self.latest_release_url())
            .header("User-Agent", REQUEST_USER_AGENT)
            .header("Accept", REQUEST_ACCEPT)
            .timeout(CHECK_TIMEOUT)
            .send()
            .await
            .map_err(|error| ReleaseError::Network(error.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| ReleaseError::Network(error.to_string()))?;

        if !status.is_success() {
            return Err(ReleaseError::Network(format!(
                "GET releases/latest returned {status}"
            )));
        }

        let release: GitHubRelease =
            serde_json::from_str(&body).map_err(|error| ReleaseError::Parse(error.to_string()))?;

        // C# `TagName` — the release workflow strips the leading `v` from the tag.
        let version = release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name)
            .to_string();

        if !version_is_newer(&version, current_version).unwrap_or(false) {
            return Ok(None);
        }

        let asset = find_installer_asset(&release.assets).ok_or_else(|| {
            ReleaseError::Parse(format!(
                "release {version} carries no installer asset ending in .exe"
            ))
        })?;

        Ok(Some(ReleaseInfo {
            version,
            asset_url: asset.browser_download_url.clone(),
            asset_name: asset.name.clone(),
        }))
    }

    /// C# `DownloadUpdatesAsync` — the installer asset to a temp path.
    pub async fn download(&self, info: &ReleaseInfo) -> Result<PathBuf, ReleaseError> {
        crate::llm::ensure_tls_provider();

        let mut response = reqwest::Client::new()
            .get(&info.asset_url)
            .header("User-Agent", REQUEST_USER_AGENT)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|error| ReleaseError::Network(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(ReleaseError::Network(format!(
                "GET {} returned {status}",
                info.asset_url
            )));
        }

        // Size check, part one: any `Content-Length` the server reports is honored
        // before a single byte is written.
        if let Some(length) = response.content_length()
            && length > MAX_INSTALLER_BYTES
        {
            return Err(ReleaseError::Io(format!(
                "installer is {length} bytes, which exceeds the {MAX_INSTALLER_BYTES} byte limit"
            )));
        }

        let installer_path = temp_installer_path(&info.version);
        // tokio's `fs` feature is not enabled in this crate, so the file is a blocking
        // `std::fs::File` written between the await points; each write is a single
        // network-sized chunk, which is far shorter than the socket read that follows.
        let mut file = std::fs::File::create(&installer_path)
            .map_err(|error| ReleaseError::Io(error.to_string()))?;

        // Size check, part two: the running total is capped too, so a server that
        // under-reports the length cannot exhaust the temp directory.
        let mut downloaded: u64 = 0;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| ReleaseError::Network(error.to_string()))?
        {
            downloaded += chunk.len() as u64;
            if downloaded > MAX_INSTALLER_BYTES {
                drop(file);
                let _ = std::fs::remove_file(&installer_path);
                return Err(ReleaseError::Io(format!(
                    "installer exceeded the {MAX_INSTALLER_BYTES} byte limit"
                )));
            }
            std::io::Write::write_all(&mut file, &chunk)
                .map_err(|error| ReleaseError::Io(error.to_string()))?;
        }

        std::io::Write::flush(&mut file).map_err(|error| ReleaseError::Io(error.to_string()))?;

        if downloaded == 0 {
            let _ = std::fs::remove_file(&installer_path);
            return Err(ReleaseError::Io("installer asset is empty".to_string()));
        }

        tracing::debug!(
            version = %info.version,
            asset = %info.asset_name,
            bytes = downloaded,
            path = %installer_path.display(),
            "installer downloaded"
        );

        Ok(installer_path)
    }

    /// C# `ApplyUpdatesAndRestart` — run the installer silently, then restart self.
    ///
    /// The relaunch cannot happen in this process: an Inno installer cannot overwrite
    /// a running `Orbital.exe`, so a detached `cmd` waits for this process to exit,
    /// installs silently, and relaunches Orbital when the installer reported success.
    pub fn apply_and_restart(&self, installer_path: &std::path::Path) -> ! {
        let exe = std::env::current_exe().ok();
        let command_line = relaunch_command_line(installer_path, exe.as_deref());

        let spawned = std::process::Command::new("cmd")
            .args(["/S", "/C", &command_line])
            .spawn();

        match spawned {
            Ok(_child) => {
                tracing::debug!(
                    "installer relaunch helper started; exiting Orbital so the update can apply"
                );
            }
            Err(error) => {
                // No shell to relaunch from: fall back to the installer alone, which is
                // the pre-relaunch behavior, so the update still applies.
                let installer = std::process::Command::new(installer_path)
                    .args(SILENT_INSTALLER_ARGS)
                    .spawn();
                tracing::error!(
                    %error,
                    path = %installer_path.display(),
                    installer_spawned = installer.is_ok(),
                    "could not start the relaunch helper"
                );
            }
        }

        std::process::exit(0);
    }

    /// C# `WaitExitThenApply(..., silent: true, restart: true)` equivalent:
    /// arm the update so it applies when the app exits.
    pub fn arm_apply_on_exit(&self, installer_path: PathBuf) {
        match ARMED_INSTALLER.get_or_init(|| Mutex::new(None)).lock() {
            Ok(mut slot) => *slot = Some(installer_path),
            Err(poisoned) => *poisoned.into_inner() = Some(installer_path),
        }
    }
}

impl Default for ReleaseManager {
    fn default() -> Self {
        Self::new(DEFAULT_REPO)
    }
}

/// The repo the source's `GithubSource` points at.
const DEFAULT_REPO: &str = "CrowKing63/Orbital";
/// `Accept: application/vnd.github+json`.
const REQUEST_ACCEPT: &str = "application/vnd.github+json";
/// GitHub requires a User-Agent on every request; the app identity is enough.
const REQUEST_USER_AGENT: &str = "Orbital";
/// The update check is best-effort, so it never blocks startup for long.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// A Velopack installer is tens of megabytes; an hour is generous.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(3_600);
/// Upper bound on a single downloaded installer.
const MAX_INSTALLER_BYTES: u64 = 512 * 1024 * 1024;
/// Inno Setup silent switches; `/NORESTART` keeps the restart decision with the app.
const SILENT_INSTALLER_ARGS: [&str; 3] = ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"];

/// GitHub's `releases/latest` payload, reduced to the fields the source reads.
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

/// A release asset (`OrbitalSetup.exe` for this project — see release.yml).
#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

impl ReleaseManager {
    /// C# `new GithubSource("https://github.com/CrowKing63/Orbital", null, false)`.
    ///
    /// `repo` is `owner/name`. A value that already carries a scheme (`http://` or
    /// `https://`) is used as the API base URL instead of `https://api.github.com`,
    /// which is what lets the wire behavior be pinned against a local test server.
    fn latest_release_url(&self) -> String {
        if self.repo.contains("://") {
            format!("{}/releases/latest", self.repo.trim_end_matches('/'))
        } else {
            format!("https://api.github.com/repos/{}/releases/latest", self.repo)
        }
    }
}

/// The installer asset the source's `GithubSource` picks for this project.
///
/// The release workflow (`release.yml`) produces a Velopack bootstrapper that it renames
/// to `OrbitalSetup.exe`, so the "Setup" spelling wins, then any name containing
/// "Orbital", then the first `.exe` at all. `RELEASES` and the portable zip are not
/// `.exe`, so they never compete.
fn find_installer_asset(assets: &[GitHubAsset]) -> Option<&GitHubAsset> {
    let executables: Vec<&GitHubAsset> = assets
        .iter()
        .filter(|asset| {
            asset
                .name
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("exe"))
        })
        .collect();

    for preference in ["setup", "orbital"] {
        if let Some(asset) = executables
            .iter()
            .find(|asset| asset.name.to_ascii_lowercase().contains(preference))
        {
            return Some(asset);
        }
    }

    executables.into_iter().next()
}

/// Unique temp path for the downloaded installer, e.g. `%TEMP%\Orbital-0.4.9-Setup.exe`.
fn temp_installer_path(version: &str) -> PathBuf {
    std::env::temp_dir().join(format!("Orbital-{version}-Setup-{}.exe", unique_suffix()))
}

/// Process-wide counter plus clock nanoseconds, so concurrent downloads never share a
/// path and no external randomness dependency is needed.
fn unique_suffix() -> String {
    static COUNTER: OnceLock<AtomicU64> = OnceLock::new();
    let counter = COUNTER
        .get_or_init(|| AtomicU64::new(0))
        .fetch_add(1, Ordering::SeqCst);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);

    format!("{}-{nanos:x}-{counter:x}", std::process::id())
}

/// Semantic compare of dotted versions ("0.4.8" < "0.4.10"); None when unparsable.
pub fn version_is_newer(candidate: &str, current: &str) -> Option<bool> {
    let candidate = version_parts(candidate)?;
    let current = version_parts(current)?;
    Some(candidate > current)
}

/// Dotted numeric compare of up to three parts; anything else is "unparsable".
///
/// A prerelease tag (`1.0.0-beta.1`) has four parts, so it compares as nothing rather
/// than as a version — the source's `GithubSource(..., prerelease: false)` skips those
/// the same way.
fn version_parts(version: &str) -> Option<[u32; 3]> {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut parts = [0u32; 3];
    for (count, part) in trimmed.split('.').enumerate() {
        if count == parts.len() {
            return None;
        }
        parts[count] = part.trim().parse().ok()?;
    }

    Some(parts)
}

/// The detached `cmd /S /C` command line behind `apply_and_restart`:
/// wait for this process to exit (two ping-seconds), run the installer silently,
/// and relaunch Orbital only when the installer reported success.
pub fn relaunch_command_line(
    installer_path: &std::path::Path,
    exe_path: Option<&std::path::Path>,
) -> String {
    let installer = cmd_quote(installer_path);
    let args = SILENT_INSTALLER_ARGS.join(" ");
    let relaunch = match exe_path {
        Some(exe) => format!("start \"\" {}", cmd_quote(exe)),
        None => "rem no executable to relaunch".to_string(),
    };
    format!("ping -n 3 127.0.0.1 >nul & {installer} {args} && {relaunch}")
}

/// Quote a path for `cmd` when it can contain spaces (installer and exe paths both
/// can, on a user machine); simple paths stay unquoted.
fn cmd_quote(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    if text.contains(' ') {
        format!("\"{text}\"")
    } else {
        text
    }
}

/// Installer armed by `arm_apply_on_exit`, applied when the app quits.
static ARMED_INSTALLER: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

/// Takes the armed installer out of the slot, so it is applied exactly once.
pub fn take_armed_installer() -> Option<PathBuf> {
    ARMED_INSTALLER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Version comparison — the source's Velopack semver window.

    #[test]
    fn version_is_newer_compares_dotted_numbers_not_strings() {
        assert_eq!(version_is_newer("0.4.10", "0.4.8"), Some(true));
        assert_eq!(version_is_newer("0.4.9", "0.4.8"), Some(true));
        assert_eq!(version_is_newer("1.0.0", "0.4.8"), Some(true));
        assert_eq!(version_is_newer("0.10.0", "0.9.9"), Some(true));
    }

    #[test]
    fn version_is_newer_reports_an_equal_or_older_release() {
        assert_eq!(version_is_newer("0.4.8", "0.4.8"), Some(false));
        assert_eq!(version_is_newer("0.4.7", "0.4.8"), Some(false));
    }

    #[test]
    fn version_is_newer_returns_none_for_unparsable_input() {
        for unparsable in [
            "",
            "  ",
            "0.4.x",
            "1.0.0-beta.1",
            "0.4.8.1",
            "v0.4.8",
            "abc",
        ] {
            assert_eq!(
                version_is_newer(unparsable, "0.4.8"),
                None,
                "'{unparsable}' should not compare"
            );
        }
    }

    #[test]
    fn version_is_newer_returns_none_when_the_current_version_is_unparsable() {
        assert_eq!(version_is_newer("0.4.9", ""), None);
    }

    #[test]
    fn version_parts_pads_a_missing_revision_with_zero() {
        assert_eq!(version_parts("0.4"), Some([0, 4, 0]));
        assert_eq!(version_parts(" 1 "), Some([1, 0, 0]));
    }

    #[test]
    fn version_parts_rejects_a_negative_part() {
        // `u32` rejects the leading `-`, which is the point: no sign games.
        assert_eq!(version_parts("-1.0.0"), None);
    }

    // URL construction.

    #[test]
    fn the_latest_release_url_points_at_githubs_api_for_an_owner_name_repo() {
        let manager = ReleaseManager::new("CrowKing63/Orbital");
        assert_eq!(
            manager.latest_release_url(),
            "https://api.github.com/repos/CrowKing63/Orbital/releases/latest"
        );
    }

    #[test]
    fn a_repo_that_carries_a_scheme_is_used_as_the_api_base() {
        // The seam the wire tests below need; not used by the app.
        let manager = ReleaseManager::new("http://127.0.0.1:9999/");
        assert_eq!(
            manager.latest_release_url(),
            "http://127.0.0.1:9999/releases/latest"
        );
    }

    // Installer asset selection.

    fn asset(name: &str) -> GitHubAsset {
        GitHubAsset {
            name: name.to_string(),
            browser_download_url: format!("https://example.test/{name}"),
        }
    }

    #[test]
    fn find_installer_asset_prefers_the_setup_name() {
        let assets = vec![
            asset("Orbital-0.4.8-Portable.zip"),
            asset("Orbital-0.4.9-full.nupkg"),
            asset("Orbital-0.4.9-Other.exe"),
            asset("OrbitalSetup.exe"),
            asset("RELEASES"),
        ];

        let picked = find_installer_asset(&assets).expect("an installer");
        assert_eq!(picked.name, "OrbitalSetup.exe");
        assert_eq!(
            picked.browser_download_url,
            "https://example.test/OrbitalSetup.exe"
        );
    }

    #[test]
    fn find_installer_asset_falls_back_to_an_orbital_name_then_any_exe() {
        let without_setup = vec![asset("RELEASES"), asset("Orbital-win.exe")];
        assert_eq!(
            find_installer_asset(&without_setup)
                .expect("an installer")
                .name,
            "Orbital-win.exe"
        );

        let any_executable = vec![asset("installer.exe"), asset("notes.txt")];
        assert_eq!(
            find_installer_asset(&any_executable)
                .expect("an installer")
                .name,
            "installer.exe"
        );
    }

    #[test]
    fn find_installer_asset_ignores_case_and_ignores_non_exe_assets() {
        let assets = vec![asset("setup.EXE")];
        assert_eq!(
            find_installer_asset(&assets).expect("an installer").name,
            "setup.EXE"
        );

        let zip_only = vec![asset("Orbital-0.4.9-Portable.zip"), asset("RELEASES")];
        assert!(find_installer_asset(&zip_only).is_none());
    }

    // The temp path.

    #[test]
    fn the_temp_installer_path_is_unique_and_lives_in_temp() {
        let first = temp_installer_path("0.4.9");
        let second = temp_installer_path("0.4.9");

        assert_eq!(first.parent(), Some(std::env::temp_dir().as_path()));
        assert_ne!(first, second, "two downloads must not share a path");
        assert_eq!(first.extension().and_then(|ext| ext.to_str()), Some("exe"));
    }

    // Wire behavior against a local HTTP server.

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    #[test]
    fn check_for_updates_returns_newer_releases_and_skips_older_ones() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        runtime().block_on(async {
            let server = MockServer::start().await;

            Mock::given(method("GET"))
                .and(path("/repos/CrowKing63/Orbital/releases/latest"))
                .and(header("User-Agent", REQUEST_USER_AGENT))
                .and(header("Accept", REQUEST_ACCEPT))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "tag_name": "v0.4.10",
                    "assets": [
                        {"name": "RELEASES", "browser_download_url": "https://example.test/RELEASES"},
                        {"name": "OrbitalSetup.exe", "browser_download_url": "https://example.test/OrbitalSetup.exe"},
                        {"name": "Orbital-0.4.10-Portable.zip", "browser_download_url": "https://example.test/portable.zip"}
                    ]
                })))
                .expect(2) // once for the newer release, once for the same release
                .mount(&server)
                .await;

            // `http://127.0.0.1:port` is the api base, so the repo path is appended.
            let manager = ReleaseManager::new(&format!("http://{}/repos/CrowKing63/Orbital", server.address()));

            let info = manager
                .check_for_updates("0.4.8")
                .await
                .expect("an update")
                .expect("the newer release");

            assert_eq!(info.version, "0.4.10", "the leading v of the tag is stripped");
            assert_eq!(info.asset_name, "OrbitalSetup.exe");

            let same = manager.check_for_updates("0.4.10").await.expect("no error");
            assert_eq!(same, None, "the release already installed is not newer");

            server.verify().await;
        });
    }

    #[test]
    fn check_for_updates_reports_a_http_failure_as_network() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        runtime().block_on(async {
            let server = MockServer::start().await;

            Mock::given(method("GET"))
                .and(path("/releases/latest"))
                .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
                .expect(1)
                .mount(&server)
                .await;

            let manager = ReleaseManager::new(&server.uri());

            let error = manager
                .check_for_updates("0.4.8")
                .await
                .expect_err("a 404 must fail");

            server.verify().await;
            assert!(
                matches!(error, ReleaseError::Network(ref message) if message.contains("404")),
                "unexpected error: {error:?}"
            );
        });
    }

    #[test]
    fn check_for_updates_reports_an_unparsable_body_as_parse() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        runtime().block_on(async {
            let server = MockServer::start().await;

            Mock::given(method("GET"))
                .and(path("/releases/latest"))
                .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
                .expect(1)
                .mount(&server)
                .await;

            let manager = ReleaseManager::new(&server.uri());

            let error = manager
                .check_for_updates("0.4.8")
                .await
                .expect_err("a non-JSON body must fail");

            server.verify().await;
            assert!(matches!(error, ReleaseError::Parse(_)));
        });
    }

    #[test]
    fn check_for_updates_reports_a_release_without_an_installer_as_parse() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        runtime().block_on(async {
            let server = MockServer::start().await;

            Mock::given(method("GET"))
                .and(path("/releases/latest"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "tag_name": "v0.4.10",
                    "assets": [
                        {"name": "RELEASES", "browser_download_url": "https://example.test/RELEASES"}
                    ]
                })))
                .expect(1)
                .mount(&server)
                .await;

            let manager = ReleaseManager::new(&server.uri());

            let error = manager
                .check_for_updates("0.4.8")
                .await
                .expect_err("no installer must fail");

            server.verify().await;
            assert!(matches!(error, ReleaseError::Parse(_)));
        });
    }

    #[test]
    fn download_streams_the_asset_to_a_unique_temp_path() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        runtime().block_on(async {
            let server = MockServer::start().await;

            let installer_bytes = b"MZ pretend installer".to_vec();
            Mock::given(method("GET"))
                .and(path("/OrbitalSetup.exe"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(installer_bytes.clone()))
                .expect(1)
                .mount(&server)
                .await;

            let manager = ReleaseManager::new(&server.uri());
            let info = ReleaseInfo {
                version: "0.4.10".to_string(),
                asset_url: format!("{}/OrbitalSetup.exe", server.uri()),
                asset_name: "OrbitalSetup.exe".to_string(),
            };

            let path = manager.download(&info).await.expect("a download");
            server.verify().await;

            let written = std::fs::read(&path).expect("the installer file");
            assert_eq!(written, installer_bytes);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn download_reports_a_failure_status_as_network() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        runtime().block_on(async {
            let server = MockServer::start().await;

            Mock::given(method("GET"))
                .and(path("/OrbitalSetup.exe"))
                .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
                .expect(1)
                .mount(&server)
                .await;

            let manager = ReleaseManager::new(&server.uri());
            let info = ReleaseInfo {
                version: "0.4.10".to_string(),
                asset_url: format!("{}/OrbitalSetup.exe", server.uri()),
                asset_name: "OrbitalSetup.exe".to_string(),
            };

            let error = manager.download(&info).await.expect_err("must fail");
            server.verify().await;
            assert!(matches!(error, ReleaseError::Network(_)));
        });
    }

    // Arming the apply-on-exit path.

    #[test]
    fn arm_apply_on_exit_can_be_taken_once() {
        // The slot is process-wide, so this fact runs once and only checks the
        // arm/take round trip rather than a shared ordering with other tests.
        let manager = ReleaseManager::new(DEFAULT_REPO);
        let armed = std::env::temp_dir().join("Orbital-test-armed.exe");
        manager.arm_apply_on_exit(armed.clone());

        assert_eq!(take_armed_installer(), Some(armed));
        assert_eq!(
            take_armed_installer(),
            None,
            "the installer is applied once"
        );
    }

    #[test]
    fn the_default_repo_matches_the_source() {
        assert_eq!(ReleaseManager::default().repo, "CrowKing63/Orbital");
        assert_eq!(DEFAULT_REPO, "CrowKing63/Orbital");
    }

    // The relaunch-after-install command line (owner decision: restart like Velopack).

    #[test]
    fn the_relaunch_command_line_waits_then_installs_then_restarts() {
        let installer =
            std::path::Path::new(r"C:\Users\dev\AppData\Local\Temp\Orbital-0.4.9-Setup.exe");
        let exe = std::path::Path::new(r"C:\Program Files\Orbital\Orbital.exe");

        let command = relaunch_command_line(installer, Some(exe));

        assert!(
            command.starts_with("ping -n 3 127.0.0.1 >nul & "),
            "{command}"
        );
        assert!(
            command
                .contains(r"C:\Users\dev\AppData\Local\Temp\Orbital-0.4.9-Setup.exe /VERYSILENT"),
            "{command}"
        );
        assert!(
            command.contains("/VERYSILENT /SUPPRESSMSGBOXES /NORESTART"),
            "{command}"
        );
        // `&&` keeps the relaunch gated on the installer's success, and the
        // space-bearing install path is quoted for `cmd`.
        assert!(
            command.contains(r#"&& start "" "C:\Program Files\Orbital\Orbital.exe""#),
            "{command}"
        );
    }

    #[test]
    fn the_relaunch_command_line_skips_space_free_quoting() {
        let command = relaunch_command_line(
            std::path::Path::new(r"C:\Temp\Orbital-Setup.exe"),
            Some(std::path::Path::new(r"C:\Orbital\Orbital.exe")),
        );

        assert!(
            command.contains(r"C:\Temp\Orbital-Setup.exe /VERYSILENT"),
            "{command}"
        );
        assert!(
            command.contains(r#"start "" C:\Orbital\Orbital.exe"#),
            "{command}"
        );
    }

    #[test]
    fn the_relaunch_command_line_without_an_executable_only_installs() {
        let command =
            relaunch_command_line(std::path::Path::new(r"C:\Temp\Orbital-Setup.exe"), None);

        assert!(
            command.contains("rem no executable to relaunch"),
            "{command}"
        );
        assert!(!command.contains("start"), "{command}");
    }
}
