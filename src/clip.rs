//! Clipboard access. Desktop: arboard. Termux/Android: termux-api commands.

#[allow(unused_imports)]
use anyhow::{bail, Context, Result};

#[cfg(not(target_os = "android"))]
pub fn get() -> Result<String> {
    let mut cb = arboard::Clipboard::new().context(
        "cannot open the clipboard (headless session? on Linux you need X11 or Wayland)",
    )?;
    cb.get_text().context("clipboard does not contain text")
}

#[cfg(not(target_os = "android"))]
pub fn set(text: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new().context("cannot open the clipboard")?;
    cb.set_text(text.to_owned())
        .context("cannot write to the clipboard")
}

#[cfg(target_os = "android")]
pub fn get() -> Result<String> {
    let out = std::process::Command::new("termux-clipboard-get")
        .output()
        .context("termux-clipboard-get not found: run `pkg install termux-api` and install the Termux:API app")?;
    if !out.status.success() {
        bail!("termux-clipboard-get failed");
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(target_os = "android")]
pub fn set(text: &str) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("termux-clipboard-set")
        .stdin(Stdio::piped())
        .spawn()
        .context("termux-clipboard-set not found: run `pkg install termux-api`")?;
    child
        .stdin
        .take()
        .context("no stdin")?
        .write_all(text.as_bytes())?;
    if !child.wait()?.success() {
        bail!("termux-clipboard-set failed");
    }
    Ok(())
}
