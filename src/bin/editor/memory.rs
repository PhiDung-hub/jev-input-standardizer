use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The last accepted payload and the draft it came from, kept per host session.
#[derive(Deserialize, Serialize)]
struct Accepted {
    payload: String,
    original: String,
}

/// A private per-session file in `$XDG_RUNTIME_DIR`, which logout clears.
pub(super) fn path(session: Option<&str>) -> Option<PathBuf> {
    let runtime = env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty())?;
    let name = format!(
        "jev-input-standardizer-{}.json",
        session.unwrap_or("default")
    );
    Some(PathBuf::from(runtime).join(name))
}

/// Re-running Alt+G on an accepted payload starts from the user's original words, so
/// appended text joins the prose instead of nesting an escaped payload.
pub(super) fn restore(draft: &str, memory: Option<&Path>) -> Option<String> {
    let previous: Accepted = serde_json::from_slice(&fs::read(memory?).ok()?).ok()?;
    restore_original(draft, &previous)
}

fn restore_original(draft: &str, previous: &Accepted) -> Option<String> {
    let rest = draft
        .trim_start()
        .strip_prefix(previous.payload.trim_end())?;
    Some(format!("{}{rest}", previous.original))
}

pub(super) fn remember(memory: &Path, payload: &str, original: &str) -> io::Result<()> {
    let bytes = serde_json::to_vec(&Accepted {
        payload: payload.to_owned(),
        original: original.to_owned(),
    })?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(memory)?.write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_the_original_draft_under_an_accepted_payload() {
        let previous = Accepted {
            payload: "<task>Audit the plugin.</task>".to_owned(),
            original: "please audit the plugin".to_owned(),
        };
        let draft = "<task>Audit the plugin.</task>\n\nKeep the intention lossless.";

        assert_eq!(
            restore_original(draft, &previous).as_deref(),
            Some("please audit the plugin\n\nKeep the intention lossless.")
        );
        assert_eq!(
            restore_original("<task>Audit the plugin.</task>", &previous).as_deref(),
            Some("please audit the plugin")
        );
        assert!(restore_original("A new prompt.", &previous).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn remembered_payload_is_private_and_restores() {
        use std::os::unix::fs::PermissionsExt;

        let memory = env::temp_dir().join(format!("jev-accepted-{}.json", std::process::id()));
        remember(&memory, "<task>x</task>", "x").unwrap();
        let mode = fs::metadata(&memory).unwrap().permissions().mode();
        let restored = restore("<task>x</task> more", Some(&memory));
        fs::remove_file(&memory).unwrap();

        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(restored.as_deref(), Some("x more"));
        assert!(restore("anything", None).is_none());
    }
}
