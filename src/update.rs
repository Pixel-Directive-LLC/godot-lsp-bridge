//! Update subcommand — download and replace the binary from the latest GitHub release.
//!
//! Fetches release metadata from the GitHub API, selects the asset for the current
//! platform, downloads and extracts it, then atomically replaces the running binary.
//! If the install directory is not already on PATH it is added automatically.

use anyhow::{Context, Result};
use std::env;
use std::io::Read;
use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;
const RELEASES_API: &str =
    "https://api.github.com/repos/Pixel-Directive-LLC/godot-lsp-bridge/releases/latest";

/// Target triple for the current build, matching the release asset naming convention.
fn current_target() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-musl"
    } else {
        "x86_64-unknown-linux-musl"
    }
}

/// Archive suffix for the current platform.
fn archive_suffix() -> &'static str {
    if cfg!(windows) {
        ".zip"
    } else {
        ".tar.gz"
    }
}

/// Download the latest release and replace the running binary.
pub async fn run() -> Result<()> {
    println!("Checking for the latest release...");

    let client = reqwest::Client::builder()
        .user_agent(concat!(
            "godot-lsp-bridge-updater/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()?;

    // Fetch latest release metadata from the GitHub API.
    let release: serde_json::Value = client
        .get(RELEASES_API)
        .send()
        .await
        .context("failed to reach GitHub API")?
        .error_for_status()
        .context("GitHub API returned an error status")?
        .json()
        .await
        .context("failed to parse GitHub API response")?;

    let tag = release["tag_name"]
        .as_str()
        .context("release metadata missing tag_name")?;

    // Short-circuit if already on the latest version.
    let current = concat!("v", env!("CARGO_PKG_VERSION"));
    if tag == current {
        println!("Already up to date ({current}).");
        return Ok(());
    }

    println!("Latest release: {tag}");

    // Locate the asset for this platform.
    let target = current_target();
    let suffix = archive_suffix();
    let asset_name = format!("godot-lsp-bridge-{target}{suffix}");

    let assets = release["assets"]
        .as_array()
        .context("release metadata missing assets array")?;

    let asset = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(&asset_name))
        .with_context(|| {
            format!("no release asset found for target {target} (looked for {asset_name})")
        })?;

    let download_url = asset["browser_download_url"]
        .as_str()
        .context("asset is missing browser_download_url")?;

    println!("Downloading {asset_name}...");

    let archive_bytes = client
        .get(download_url)
        .send()
        .await
        .context("download request failed")?
        .error_for_status()
        .context("download returned an error status")?
        .bytes()
        .await
        .context("failed to read download body")?;

    // Extract the binary from the downloaded archive.
    let binary_name = if cfg!(windows) {
        "godot-lsp-bridge.exe"
    } else {
        "godot-lsp-bridge"
    };

    println!("Extracting {binary_name}...");
    let new_binary = extract_binary(&archive_bytes, binary_name)?;

    // Determine where the running binary lives.
    let exe_path = env::current_exe().context("could not determine the current executable path")?;
    let install_dir = exe_path
        .parent()
        .context("executable has no parent directory")?
        .to_path_buf();

    replace_binary(&exe_path, &install_dir, binary_name, &new_binary)?;

    println!("Updated to {tag}.");

    // Ensure the install directory is on PATH so the updated binary is callable.
    ensure_path(&install_dir)
}

/// Write `new_binary` to a temp path then atomically rename it over `exe_path`.
///
/// On Windows the running executable cannot be overwritten directly, so the old
/// binary is first renamed to `<name>.old` before the replacement is moved into place.
fn replace_binary(
    exe_path: &Path,
    install_dir: &Path,
    binary_name: &str,
    new_binary: &[u8],
) -> Result<()> {
    let temp_path = install_dir.join(format!("{binary_name}.tmp"));
    std::fs::write(&temp_path, new_binary).context("failed to write temporary binary to disk")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o755))
            .context("failed to make temporary binary executable")?;
    }

    // On Windows the running exe cannot be replaced in place — rename it first.
    #[cfg(windows)]
    {
        let old_path = install_dir.join("godot-lsp-bridge.old.exe");
        let _ = std::fs::remove_file(&old_path); // ignore if already absent
        std::fs::rename(exe_path, &old_path)
            .context("failed to rename old binary before replacement")?;
    }

    std::fs::rename(&temp_path, exe_path).context("failed to move new binary into place")?;

    Ok(())
}

/// Extract the named binary from the archive bytes (.tar.gz on Unix, .zip on Windows).
fn extract_binary(data: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    if cfg!(windows) {
        extract_from_zip(data, binary_name)
    } else {
        extract_from_tgz(data, binary_name)
    }
}

fn extract_from_tgz(data: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(gz);

    for entry in archive.entries().context("failed to read tar entries")? {
        let mut entry = entry.context("corrupt tar entry")?;
        let path = entry.path().context("tar entry has no path")?.to_path_buf();

        if path.file_name().and_then(|n| n.to_str()) == Some(binary_name) {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .context("failed to read binary from tar")?;
            return Ok(buf);
        }
    }

    anyhow::bail!(
        "binary {binary_name} not found in the downloaded archive; \
         the release asset may be malformed"
    )
}

fn extract_from_zip(data: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).context("failed to open zip archive")?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).context("failed to read zip entry")?;
        if std::path::Path::new(file.name())
            .file_name()
            .and_then(|n| n.to_str())
            == Some(binary_name)
        {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)
                .context("failed to read binary from zip")?;
            return Ok(buf);
        }
    }

    anyhow::bail!(
        "binary {binary_name} not found in the downloaded archive; \
         the release asset may be malformed"
    )
}

/// Ensure `dir` is present on the user's PATH, appending it if needed.
fn ensure_path(dir: &Path) -> Result<()> {
    #[cfg(windows)]
    return ensure_path_windows(dir);

    #[cfg(not(windows))]
    return ensure_path_unix(dir);
}

/// Add `dir` to the Windows User PATH via PowerShell's `[Environment]::SetEnvironmentVariable`.
#[cfg(windows)]
fn ensure_path_windows(dir: &Path) -> Result<()> {
    let dir_str = dir.to_string_lossy();

    // PowerShell: read User PATH, append dir if absent, write back.
    let script = format!(
        r#"
$p = [Environment]::GetEnvironmentVariable('PATH', 'User')
if ($p -notlike '*{dir_str}*') {{
    [Environment]::SetEnvironmentVariable('PATH', "$p;{dir_str}", 'User')
    Write-Host "Added {dir_str} to User PATH. Restart your terminal to apply."
}} else {{
    Write-Host "{dir_str} is already on User PATH."
}}
"#
    );

    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .context("failed to launch PowerShell for PATH update")?;

    if !status.success() {
        anyhow::bail!("PowerShell PATH update exited with status {status}");
    }

    Ok(())
}

/// Append an `export PATH` line for `dir` to the user's shell rc file if not already present.
#[cfg(not(windows))]
fn ensure_path_unix(dir: &Path) -> Result<()> {
    let dir_str = dir.to_string_lossy();

    // Already on the active PATH — nothing to do.
    if std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|p| p == dir_str.as_ref())
    {
        println!("{dir_str} is already on PATH.");
        return Ok(());
    }

    let home = dirs::home_dir().context("could not determine home directory")?;

    // Pick the rc file: prefer .zshrc on macOS (default since Catalina), .bashrc on Linux.
    let rc_file = select_rc_file(&home);

    let export_line = format!("\nexport PATH=\"$PATH:{dir_str}\"\n");

    // Skip if already written.
    if rc_file.exists() {
        let contents = std::fs::read_to_string(&rc_file).context("failed to read shell rc file")?;
        if contents.contains(dir_str.as_ref()) {
            println!("{dir_str} is already on PATH.");
            return Ok(());
        }
    }

    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&rc_file)
        .with_context(|| format!("failed to open {} for writing", rc_file.display()))?;

    file.write_all(export_line.as_bytes())
        .with_context(|| format!("failed to write PATH export to {}", rc_file.display()))?;

    println!(
        "Added {dir_str} to PATH in {}.\nRun `source {}` or restart your terminal to apply.",
        rc_file.display(),
        rc_file.display()
    );

    Ok(())
}

/// Select the most appropriate shell rc file for the current platform/user.
#[cfg(not(windows))]
fn select_rc_file(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        // macOS default shell is zsh since Catalina.
        let zshrc = home.join(".zshrc");
        let bash_profile = home.join(".bash_profile");
        if zshrc.exists() || !bash_profile.exists() {
            zshrc
        } else {
            bash_profile
        }
    } else {
        // Linux: prefer .bashrc; fall back to .zshrc if that's what exists.
        let bashrc = home.join(".bashrc");
        let zshrc = home.join(".zshrc");
        if bashrc.exists() || !zshrc.exists() {
            bashrc
        } else {
            zshrc
        }
    }
}
