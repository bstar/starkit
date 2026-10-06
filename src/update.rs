//! Shared GitHub updater. Applications name their repository and release flavor.
//! Network and staging run on an application-owned worker. Activation is a
//! separate startup step, so a running terminal/session is never interrupted.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_METADATA: u64 = 2 * 1024 * 1024;
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
const MAX_EXECUTABLE: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub enum Package {
    MacArchive,
    AppImage,
}

/// A release flavor is a distinct asset prefix, never a fallback. A graphical
/// application must not silently install a release that lacks its renderer.
#[derive(Clone, Debug)]
pub struct Application {
    pub owner: String,
    pub repository: String,
    pub executable: String,
    pub asset_prefix: String,
    pub version: String,
    pub package: Package,
    pub architecture: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Candidate {
    pub tag: String,
    pub version: String,
    pub asset: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub release_url: String,
    #[serde(default)]
    pub release_notes: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
}

#[derive(Deserialize, Serialize)]
struct Pending {
    target: PathBuf,
    candidate: Candidate,
    executable_sha256: String,
    executable_size: u64,
}

/// The standalone release default is automatic; package/source installs are
/// rejected independently of this preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Automatic,
    Notify,
    Off,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Status {
    pub last_checked: Option<u64>,
    pub available: Option<Candidate>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct Updater {
    app: Application,
    cache: PathBuf,
    agent: ureq::Agent,
}

impl Updater {
    pub fn new(app: Application, cache: PathBuf) -> Result<Self> {
        for value in [
            &app.owner,
            &app.repository,
            &app.executable,
            &app.asset_prefix,
        ] {
            ensure!(identifier(value), "Invalid updater application identifier");
        }
        Version::parse(&app.version)?;
        ensure!(
            matches!(app.architecture.as_str(), "aarch64" | "x86_64"),
            "Unsupported update architecture"
        );
        let agent = crate::net::builder(&format!("{}/{} updater", app.executable, app.version))
            .https_only(true)
            .timeout_global(Some(Duration::from_secs(180)))
            .build()
            .into();
        Ok(Self { app, cache, agent })
    }

    pub fn policy(&self) -> Result<Policy> {
        let path = self.cache.join("policy.json");
        match File::open(path) {
            Ok(file) => Ok(serde_json::from_slice(&bounded(file, MAX_METADATA)?)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Policy::Automatic),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_policy(&self, policy: Policy) -> Result<()> {
        let _lock = self.lock()?;
        write_json(&self.cache.join("policy.json"), &policy)?;
        if policy == Policy::Off {
            self.clear_pending()?;
        }
        Ok(())
    }

    pub fn status(&self) -> Result<Status> {
        match File::open(self.cache.join("status.json")) {
            Ok(file) => Ok(serde_json::from_slice(&bounded(file, MAX_METADATA)?)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Status::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Start a detached worker after startup activation. It polls at most once
    /// every six hours across application instances. Errors are saved for the
    /// application's status UI/command, never written over the terminal.
    pub fn spawn_background(&self, target: PathBuf) -> Result<()> {
        standalone_target(&target)?;
        if self.policy()? == Policy::Off {
            return Ok(());
        }
        let updater = self.clone();
        std::thread::Builder::new()
            .name(format!("{}-update", self.app.executable))
            .spawn(move || loop {
                if let Err(error) = updater.background_once(&target) {
                    tracing::warn!("Automatic update: {error:#}");
                }
                std::thread::sleep(Duration::from_secs(3600));
            })?;
        Ok(())
    }

    fn background_once(&self, target: &Path) -> Result<()> {
        if self.policy()? == Policy::Off {
            return Ok(());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let mut status = {
            let _lock = self.lock()?;
            let mut status = self.status()?;
            if status
                .last_checked
                .is_some_and(|last| now.saturating_sub(last) < 6 * 3600)
            {
                return Ok(());
            }
            status.last_checked = Some(now);
            write_json(&self.cache.join("status.json"), &status)?;
            status
        };
        let result = (|| -> Result<()> {
            status.available = self.check()?;
            if self.policy()? == Policy::Automatic {
                if let Some(candidate) = &status.available {
                    self.stage(candidate, target)?;
                }
            }
            Ok(())
        })();
        status.error = result.as_ref().err().map(|error| format!("{error:#}"));
        let _lock = self.lock()?;
        write_json(&self.cache.join("status.json"), &status)?;
        result
    }

    /// Published stable releases only. Drafts/prereleases and equal or older
    /// versions cannot become an update. Assets must match this exact flavor.
    pub fn check(&self) -> Result<Option<Candidate>> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            self.app.owner, self.app.repository
        );
        let mut response = self
            .agent
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        ensure!(
            response.status().is_success(),
            "GitHub release check returned {}",
            response.status()
        );
        let bytes = bounded(response.body_mut().as_reader(), MAX_METADATA)?;
        let release: Release = serde_json::from_slice(&bytes)?;
        let Some(asset) = select_asset(&self.app, &release)? else {
            return Ok(None);
        };
        let sums = release
            .assets
            .iter()
            .find(|a| a.name == "SHA256SUMS")
            .context("Release is missing SHA256SUMS")?;
        self.asset_url(&sums.browser_download_url, &release.tag_name)?;
        let sums = self.fetch(&sums.browser_download_url, MAX_METADATA)?;
        let sha256 = checksum(&sums, &asset.name)?;
        Ok(Some(Candidate {
            tag: release.tag_name.clone(),
            version: release.tag_name.trim_start_matches('v').into(),
            asset: asset.name.clone(),
            url: asset.browser_download_url.clone(),
            size: asset.size,
            sha256,
            release_url: release.html_url,
            release_notes: release.body.unwrap_or_default(),
        }))
    }

    /// Stage a verified executable without replacing the running application.
    /// Takes a process lock shared with activation/rollback; partial downloads
    /// remain private temporaries and never become a pending installation.
    pub fn stage(&self, candidate: &Candidate, target: &Path) -> Result<()> {
        let _lock = self.lock()?;
        let target = standalone_target(target)?;
        ensure!(
            Version::parse(&candidate.version)? > Version::parse(&self.app.version)?,
            "Update must be newer than the running application"
        );
        ensure!(
            candidate.asset == asset_name(&self.app, &candidate.version),
            "Update asset does not match this application"
        );
        ensure!(
            Version::parse(candidate.tag.strip_prefix('v').unwrap_or(&candidate.tag))?
                == Version::parse(&candidate.version)?,
            "Release tag/version mismatch"
        );
        self.asset_url(&candidate.url, &candidate.tag)?;
        ensure!(
            candidate.size > 0 && candidate.size <= MAX_DOWNLOAD,
            "Invalid update asset size"
        );
        let mut download = tempfile::NamedTempFile::new_in(&self.cache)?;
        let mut response = self.agent.get(&candidate.url).call()?;
        ensure!(
            response.status().is_success(),
            "Update download returned {}",
            response.status()
        );
        let mut reader = response.body_mut().as_reader().take(candidate.size + 1);
        let copied = std::io::copy(&mut reader, download.as_file_mut())?;
        ensure!(copied == candidate.size, "Update download size mismatch");
        ensure!(
            hash_file(download.path())? == candidate.sha256,
            "Update checksum mismatch"
        );
        let mut staged = tempfile::NamedTempFile::new_in(&self.cache)?;
        match self.app.package {
            Package::AppImage => {
                std::io::copy(&mut File::open(download.path())?, staged.as_file_mut())?;
            }
            Package::MacArchive => extract_executable(
                File::open(download.path())?,
                &self.app.executable,
                staged.as_file_mut(),
            )?,
        }
        staged
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))?;
        staged.as_file().sync_all()?;
        verify_executable(
            staged.path(),
            &self.app.executable,
            &candidate.version,
            self.app.package,
        )?;
        let pending = Pending {
            target,
            candidate: candidate.clone(),
            executable_sha256: hash_file(staged.path())?,
            executable_size: staged.as_file().metadata()?.len(),
        };
        // Remove old metadata first: a crash must not pair the new executable
        // with an older candidate's metadata.
        remove_missing_ok(&self.cache.join("pending.json"))?;
        staged.persist(self.cache.join("pending.bin"))?;
        write_json(&self.cache.join("pending.json"), &pending)?;
        Ok(())
    }

    /// Call before terminal setup. Returns true when the caller should exec
    /// its new installation with the same arguments. Never performs networking.
    pub fn activate(&self, target: &Path) -> Result<bool> {
        let _lock = self.lock()?;
        let Some(pending) = self.pending()? else {
            return Ok(false);
        };
        let target = standalone_target(target)?;
        ensure!(
            pending.target == target,
            "Pending update belongs to a different installation"
        );
        if Version::parse(&pending.candidate.version)? <= Version::parse(&self.app.version)? {
            self.clear_pending()?;
            return Ok(false);
        }
        let source = self.cache.join("pending.bin");
        ensure!(
            fs::metadata(&source)?.len() == pending.executable_size
                && hash_file(&source)? == pending.executable_sha256,
            "Staged executable has changed"
        );
        ensure!(
            pending.candidate.asset == asset_name(&self.app, &pending.candidate.version),
            "Pending update is for a different build flavor"
        );
        replace(&source, &target, true)?;
        write_json(&self.cache.join("applied.json"), &pending.candidate)?;
        self.clear_pending()?;
        Ok(true)
    }

    /// Receipt survives replacement and exec so the new UI can explain the update.
    pub fn applied(&self) -> Result<Option<Candidate>> {
        match File::open(self.cache.join("applied.json")) {
            Ok(file) => Ok(Some(serde_json::from_slice(&bounded(file, MAX_METADATA)?)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn pending_version(&self) -> Result<Option<String>> {
        Ok(self.pending()?.map(|p| p.candidate.version))
    }

    /// Restore the prior installation. The current process keeps running;
    /// the caller can exec the restored executable before opening a terminal.
    pub fn rollback(&self, target: &Path) -> Result<()> {
        let _lock = self.lock()?;
        let target = standalone_target(target)?;
        let backup = backup_path(&target);
        ensure!(backup.is_file(), "No previous installation is available");
        replace(&backup, &target, false)?;
        self.clear_pending()
    }

    fn asset_url(&self, url: &str, tag: &str) -> Result<()> {
        let prefix = format!(
            "https://github.com/{}/{}/releases/download/{tag}/",
            self.app.owner, self.app.repository
        );
        ensure!(
            url.strip_prefix(&prefix).is_some_and(identifier),
            "Release asset URL is outside the application's GitHub repository"
        );
        Ok(())
    }
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        let mut response = self.agent.get(url).call()?;
        ensure!(
            response.status().is_success(),
            "Release download returned {}",
            response.status()
        );
        bounded(response.body_mut().as_reader(), limit)
    }
    fn lock(&self) -> Result<File> {
        fs::create_dir_all(&self.cache)?;
        fs::set_permissions(&self.cache, fs::Permissions::from_mode(0o700))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.cache.join("lock"))?;
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .context("Another update operation is already running")?;
        // A killed updater can leave unnamed staging downloads behind. This
        // directory is updater-owned and the exclusive lock proves no other
        // process is using these temporary files.
        for entry in fs::read_dir(&self.cache)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(".tmp") {
                remove_missing_ok(&entry.path())?;
            }
        }
        if !self.cache.join("pending.json").exists() {
            remove_missing_ok(&self.cache.join("pending.bin"))?;
        }
        Ok(file)
    }
    fn pending(&self) -> Result<Option<Pending>> {
        let path = self.cache.join("pending.json");
        match File::open(path) {
            Ok(file) => Ok(Some(serde_json::from_slice(&bounded(file, MAX_METADATA)?)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn clear_pending(&self) -> Result<()> {
        remove_missing_ok(&self.cache.join("pending.json"))?;
        remove_missing_ok(&self.cache.join("pending.bin"))
    }
}

/// Resolves command symlinks but rejects build trees, package-manager stores,
/// system installations, and hard-linked executables. The updater only owns
/// a user's writable standalone release binary, never its source/package.
pub fn standalone_target(path: &Path) -> Result<PathBuf> {
    let target = fs::canonicalize(path)?;
    ensure!(target.is_file(), "Update target is not a file");
    ensure!(
        !target.starts_with("/nix/store")
            && !target.starts_with("/usr")
            && !target.starts_with("/opt/homebrew")
            && !target.starts_with("/opt/local")
            && !target.starts_with("/Applications"),
        "Use this installation's package manager to update it"
    );
    for parent in target.ancestors().skip(1) {
        ensure!(!(parent.join("Cargo.toml").is_file() && parent.join("src").is_dir()), "Source checkouts are updated through Git and rebuilt, not replaced with release downloads");
    }
    use std::os::unix::fs::MetadataExt;
    ensure!(
        fs::metadata(&target)?.nlink() == 1,
        "Hard-linked installations cannot self-update"
    );
    let parent = target.parent().context("Update target has no directory")?;
    // Actual creation, rather than a mode-bit guess (ACLs matter on macOS).
    let _probe = tempfile::NamedTempFile::new_in(parent)
        .context("Installation directory is not writable")?;
    Ok(target)
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}
fn asset_name(app: &Application, version: &str) -> String {
    match app.package {
        Package::MacArchive => format!(
            "{}-{version}-{}-apple-darwin.tar.gz",
            app.asset_prefix, app.architecture
        ),
        Package::AppImage => format!(
            "{}-{version}-{}.AppImage",
            app.asset_prefix, app.architecture
        ),
    }
}
fn select_asset<'a>(app: &Application, release: &'a Release) -> Result<Option<&'a Asset>> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )?;
    if !version.pre.is_empty() || version <= Version::parse(&app.version)? {
        return Ok(None);
    }
    let name = asset_name(app, &version.to_string());
    let matches: Vec<_> = release.assets.iter().filter(|a| a.name == name).collect();
    ensure!(matches.len() <= 1, "Duplicate release asset");
    let Some(asset) = matches.first() else {
        return Ok(None);
    };
    ensure!(
        asset.size > 0 && asset.size <= MAX_DOWNLOAD,
        "Invalid release asset size"
    );
    let prefix = format!(
        "https://github.com/{}/{}/releases/download/{}/",
        app.owner, app.repository, release.tag_name
    );
    ensure!(
        asset.browser_download_url == format!("{prefix}{name}"),
        "Invalid release asset URL"
    );
    Ok(Some(*asset))
}
fn checksum(bytes: &[u8], name: &str) -> Result<String> {
    let mut found = None;
    for line in std::str::from_utf8(bytes)?.lines() {
        let Some((hash, file)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if file.trim_start().trim_start_matches('*') == name {
            ensure!(found.is_none(), "Duplicate release checksum");
            ensure!(
                hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid SHA-256 checksum"
            );
            found = Some(hash.to_ascii_lowercase());
        }
    }
    found.context("Release checksum is missing for the selected asset")
}
fn bounded(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Release response exceeds size limit"
    );
    Ok(bytes)
}
fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn extract_executable(reader: impl Read, executable: &str, out: &mut File) -> Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(reader));
    let mut found = false;
    let mut total = 0u64;
    for (count, entry) in archive.entries()?.enumerate() {
        ensure!(count < 4096, "Update archive contains too many entries");
        let mut entry = entry?;
        total = total
            .checked_add(entry.size())
            .context("Archive size overflow")?;
        ensure!(
            total <= MAX_EXECUTABLE,
            "Update archive exceeds unpacked size limit"
        );
        let path = entry.path()?;
        ensure!(
            path.components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "Unsafe update archive path"
        );
        let kind = entry.header().entry_type();
        ensure!(
            kind.is_file() || kind.is_dir(),
            "Update archive contains a link or special file"
        );
        if path.file_name().is_some_and(|name| name == executable) && kind.is_file() {
            ensure!(!found, "Update archive contains multiple executables");
            found = true;
            ensure!(
                entry.size() > 0 && entry.size() <= MAX_EXECUTABLE,
                "Invalid executable size"
            );
            let expected = entry.size();
            ensure!(
                std::io::copy(&mut entry, out)? == expected,
                "Truncated update executable"
            );
        }
    }
    ensure!(found, "Update archive is missing the executable");
    Ok(())
}
fn verify_executable(path: &Path, name: &str, version: &str, package: Package) -> Result<()> {
    let output = tempfile::tempfile()?;
    let mut command = Command::new(path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(Stdio::null());
    if matches!(package, Package::AppImage) {
        command
            .env("APPIMAGE_EXTRACT_AND_RUN", "1")
            .env_remove("APPIMAGE")
            .env_remove("APPDIR");
    }
    let mut child = command
        .spawn()
        .context("Downloaded executable cannot start on this machine")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "Downloaded executable failed its version check"
            );
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Downloaded executable version check timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    use std::io::{Seek, SeekFrom};
    let mut output = output;
    output.seek(SeekFrom::Start(0))?;
    let bytes = bounded(output, 4096)?;
    ensure!(
        std::str::from_utf8(&bytes)?.trim() == format!("{name} {version}"),
        "Downloaded executable version does not match the release"
    );
    Ok(())
}
fn backup_path(target: &Path) -> PathBuf {
    target.with_file_name(format!(
        ".{}.previous",
        target.file_name().unwrap().to_string_lossy()
    ))
}
fn replace(source: &Path, target: &Path, backup: bool) -> Result<()> {
    let parent = target.parent().context("Installation has no parent")?;
    let mut next = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut File::open(source)?, next.as_file_mut())?;
    next.as_file()
        .set_permissions(fs::Permissions::from_mode(0o755))?;
    next.as_file().sync_all()?;
    if backup {
        let mut previous = tempfile::NamedTempFile::new_in(parent)?;
        std::io::copy(&mut File::open(target)?, previous.as_file_mut())?;
        previous
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))?;
        previous.as_file().sync_all()?;
        previous.persist(backup_path(target))?;
    }
    next.persist(target)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().context("Missing state directory")?)?;
    temp.as_file_mut()
        .write_all(&serde_json::to_vec_pretty(value)?)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
fn remove_missing_ok(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> Application {
        Application {
            owner: "bstar".into(),
            repository: "starfold".into(),
            executable: "starfold".into(),
            asset_prefix: "starfold".into(),
            version: "0.0.2".into(),
            package: Package::MacArchive,
            architecture: "aarch64".into(),
        }
    }
    fn release(tag: &str, prefix: &str) -> Release {
        let name = format!(
            "{prefix}-{}-aarch64-apple-darwin.tar.gz",
            tag.trim_start_matches('v')
        );
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            html_url: "https://github.com/bstar/starfold/releases".into(),
            body: Some("Release notes".into()),
            assets: vec![Asset {
                name: name.clone(),
                size: 10,
                browser_download_url: format!(
                    "https://github.com/bstar/starfold/releases/download/{tag}/{name}"
                ),
            }],
        }
    }
    #[test]
    fn stable_newer_releases_must_match_platform_and_flavor() {
        assert!(select_asset(&app(), &release("v0.0.3", "starfold"))
            .unwrap()
            .is_some());
        for tag in ["v0.0.1", "v0.0.2", "v0.0.3-beta.1"] {
            assert!(select_asset(&app(), &release(tag, "starfold"))
                .unwrap()
                .is_none());
        }
        let mut graphical = app();
        graphical.asset_prefix = "starfold-graphical".into();
        assert!(select_asset(&graphical, &release("v0.0.3", "starfold"))
            .unwrap()
            .is_none());
        let mut wrong_arch = app();
        wrong_arch.architecture = "x86_64".into();
        assert!(select_asset(&wrong_arch, &release("v0.0.3", "starfold"))
            .unwrap()
            .is_none());
        let mut r = release("v0.0.3", "starfold");
        r.draft = true;
        assert!(select_asset(&app(), &r).unwrap().is_none());
        r.draft = false;
        r.prerelease = true;
        assert!(select_asset(&app(), &r).unwrap().is_none());
        r.prerelease = false;
        r.assets[0].browser_download_url = "https://evil.example/starfold".into();
        assert!(select_asset(&app(), &r).is_err());
    }
    #[test]
    fn checksum_requires_an_exact_unique_filename() {
        let hash = "ab".repeat(32);
        assert_eq!(
            checksum(format!("{hash}  file\n").as_bytes(), "file").unwrap(),
            hash
        );
        assert!(checksum(format!("{hash}  other\n").as_bytes(), "file").is_err());
        assert!(checksum(format!("{hash}  file\n{hash}  file\n").as_bytes(), "file").is_err());
        assert!(checksum(b"nope  file", "file").is_err());
        assert!(bounded(&b"12345"[..], 4).is_err());
    }
    fn archive(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (path, bytes, link) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_mode(0o755);
            if *link {
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_link_name("outside").unwrap();
                h.set_size(0);
            } else {
                h.set_size(bytes.len() as u64);
            }
            h.set_cksum();
            archive.append_data(&mut h, path, *bytes).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap()
    }
    #[test]
    fn archives_require_one_regular_executable_and_reject_links() {
        let mut out = tempfile::tempfile().unwrap();
        extract_executable(
            &archive(&[("release/starfold", b"binary", false)])[..],
            "starfold",
            &mut out,
        )
        .unwrap();
        for entries in [
            vec![("release/README", &b"text"[..], false)],
            vec![
                ("a/starfold", &b"one"[..], false),
                ("b/starfold", &b"two"[..], false),
            ],
            vec![("release/starfold", &b""[..], true)],
        ] {
            assert!(extract_executable(
                &archive(&entries)[..],
                "starfold",
                &mut tempfile::tempfile().unwrap()
            )
            .is_err());
        }
    }
    fn pending_fixture() -> (tempfile::TempDir, Updater, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("starfold");
        fs::write(&target, b"old executable").unwrap();
        let updater = Updater::new(app(), dir.path().join("updates")).unwrap();
        {
            let _lock = updater.lock().unwrap();
        }
        let staged = updater.cache.join("pending.bin");
        fs::write(&staged, b"new executable").unwrap();
        let p = Pending {
            target: fs::canonicalize(&target).unwrap(),
            candidate: Candidate {
                tag: "v0.0.3".into(),
                version: "0.0.3".into(),
                asset: asset_name(&app(), "0.0.3"),
                url: String::new(),
                size: 10,
                sha256: "ab".repeat(32),
                release_url: String::new(),
                release_notes: "Improved playback".into(),
            },
            executable_sha256: hash_file(&staged).unwrap(),
            executable_size: 14,
        };
        write_json(&updater.cache.join("pending.json"), &p).unwrap();
        (dir, updater, target)
    }
    #[test]
    fn activation_preserves_command_symlinks_and_rollback_restores_old_bytes() {
        let (dir, updater, target) = pending_fixture();
        let command = dir.path().join("command");
        std::os::unix::fs::symlink(&target, &command).unwrap();
        assert!(updater.activate(&command).unwrap());
        assert_eq!(fs::read(&command).unwrap(), b"new executable");
        assert!(fs::symlink_metadata(&command).unwrap().is_symlink());
        assert!(updater.pending_version().unwrap().is_none());
        updater.rollback(&command).unwrap();
        assert_eq!(fs::read(&command).unwrap(), b"old executable");
    }
    #[test]
    fn tampering_wrong_installation_and_disabling_cannot_activate_pending_bytes() {
        let (_dir, updater, target) = pending_fixture();
        fs::write(updater.cache.join("pending.bin"), b"bad executable").unwrap();
        assert!(updater.activate(&target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old executable");
        let (_dir, updater, target) = pending_fixture();
        let other = target.with_file_name("other");
        fs::write(&other, b"other").unwrap();
        assert!(updater.activate(&other).is_err());
        assert_eq!(fs::read(&other).unwrap(), b"other");
        updater.set_policy(Policy::Off).unwrap();
        assert!(!updater.activate(&target).unwrap());
    }
    #[test]
    fn source_checkouts_and_concurrent_writers_are_protected() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        let target = dir.path().join("starfold");
        fs::write(&target, "binary").unwrap();
        assert!(standalone_target(&target).is_err());
        let (_dir, updater, _) = pending_fixture();
        let lock = updater.lock().unwrap();
        assert!(updater.lock().is_err());
        drop(lock);
        assert!(updater.lock().is_ok());
    }
    proptest::proptest! {
        #[test]
        fn foreign_checksum_text_never_panics(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..4096), name in ".{0,128}") { let _ = checksum(&bytes, &name); }
        #[test]
        fn foreign_archives_never_panic(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..4096)) { let _ = extract_executable(&bytes[..], "starfold", &mut tempfile::tempfile().unwrap()); }
    }
}
