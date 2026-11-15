use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DRAFT_FILE: &str = "compose.md";

const FALLBACKS: [&str; 4] = ["nano", "nvim", "vim", "vi"];

const WINDOWS_FALLBACKS: [&str; 3] = ["nvim", "vim", "nano"];

const IMMEDIATE_LIMIT: Duration = Duration::from_secs(1);

pub const RETURNED_IMMEDIATELY: &str = "the editor returned immediately; set EDITOR to a command that waits, for example `code --wait`";

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Apply,
    ReturnedImmediately,
}

pub fn judge(elapsed: Duration, changed: bool) -> Outcome {
    if elapsed < IMMEDIATE_LIMIT && !changed {
        Outcome::ReturnedImmediately
    } else {
        Outcome::Apply
    }
}

pub fn unchanged(original: &str, edited: &str) -> bool {
    let trim = |t: &str| t.trim_end_matches(['\r', '\n']).to_owned();
    trim(original) == trim(edited)
}

pub fn editor_command(
    visual: Option<String>,
    editor: Option<String>,
    available: impl Fn(&str) -> bool,
) -> Result<Vec<String>, String> {
    let pick = [visual, editor]
        .into_iter()
        .flatten()
        .find(|v| !v.trim().is_empty());
    if let Some(cmd) = pick {
        return Ok(cmd.split_whitespace().map(str::to_owned).collect());
    }
    if cfg!(windows) {
        let found = WINDOWS_FALLBACKS.into_iter().find(|name| available(name));
        return Ok(vec![found.unwrap_or("notepad").to_owned()]);
    }
    FALLBACKS
        .into_iter()
        .find(|name| available(name))
        .map(|name| vec![name.to_owned()])
        .ok_or_else(|| "set $EDITOR to write messages in an editor (nano, nvim, vim and vi were not found)".to_owned())
}

pub fn write_draft(dir: &Path, text: &str) -> Result<PathBuf, String> {
    make_private_dir(dir)?;
    let path = dir.join(DRAFT_FILE);
    write_private_file(&path, text)?;
    Ok(path)
}

#[cfg(unix)]
fn make_private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .and_then(|()| std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)))
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))
}

#[cfg(not(unix))]
fn make_private_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))
}

#[cfg(unix)]
fn write_private_file(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .and_then(|()| file.write_all(text.as_bytes()))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn read_back(path: &Path) -> Result<String, String> {
    let mut text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    std::fs::remove_file(path).map_err(|e| {
        format!(
            "cannot remove {}: {e}; the edited text is still in that file",
            path.display()
        )
    })?;
    Ok(text)
}

pub fn run_on_file(command: &[String], path: &Path) -> Result<Duration, String> {
    let Some((program, args)) = command.split_first() else {
        return Err("no editor command".to_owned());
    };
    let started = Instant::now();
    let status = std::process::Command::new(program)
        .args(args)
        .arg(path)
        .status()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    if status.success() {
        Ok(started.elapsed())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gramtty-editor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn some(s: &str) -> Option<String> {
        Some(s.to_owned())
    }

    #[test]
    fn visual_wins_over_editor_and_is_split_without_a_shell() {
        assert_eq!(
            editor_command(some("code --wait"), some("vi"), |_| false),
            Ok(vec!["code".to_owned(), "--wait".to_owned()])
        );
        assert_eq!(editor_command(None, some("  nano  "), |_| false), Ok(vec!["nano".to_owned()]));
        assert_eq!(editor_command(some(""), some("vi"), |_| false), Ok(vec!["vi".to_owned()]));
        assert_eq!(editor_command(some("  "), some("vi"), |_| false), Ok(vec!["vi".to_owned()]));
    }

    #[test]
    fn nothing_set_falls_back_to_an_installed_editor() {
        let got = editor_command(None, some(""), |name| name == "nvim" || name == "vi");
        let none = editor_command(None, None, |_| false);
        if cfg!(windows) {
            assert_eq!(got, Ok(vec!["nvim".to_owned()]));
            assert_eq!(none, Ok(vec!["notepad".to_owned()]));
        } else {
            assert_eq!(got, Ok(vec!["nvim".to_owned()]));
            assert_eq!(
                editor_command(None, None, |name| name != "nvim"),
                Ok(vec!["nano".to_owned()])
            );
            assert!(none.unwrap_err().starts_with("set $EDITOR"));
        }
    }

    #[test]
    fn windows_prefers_nvim_vim_nano_then_notepad() {
        let pick = |have: &[&str]| editor_command(None, None, |n| have.contains(&n));
        if cfg!(windows) {
            assert_eq!(pick(&["nano", "vim", "nvim"]), Ok(vec!["nvim".to_owned()]));
            assert_eq!(pick(&["nano", "vim"]), Ok(vec!["vim".to_owned()]));
            assert_eq!(pick(&["nano", "vi"]), Ok(vec!["nano".to_owned()]));
            assert_eq!(pick(&["vi"]), Ok(vec!["notepad".to_owned()]));
            assert_eq!(pick(&[]), Ok(vec!["notepad".to_owned()]));
        } else {
            assert_eq!(pick(&["nano", "vim", "nvim"]), Ok(vec!["nano".to_owned()]));
            assert_eq!(pick(&["vim", "nvim"]), Ok(vec!["nvim".to_owned()]));
        }
    }

    #[test]
    fn an_instant_return_without_changes_keeps_the_draft() {
        let fast = Duration::from_millis(200);
        let slow = Duration::from_secs(3);
        assert_eq!(judge(fast, false), Outcome::ReturnedImmediately);
        assert_eq!(judge(fast, true), Outcome::Apply);
        assert_eq!(judge(slow, false), Outcome::Apply);
        assert_eq!(judge(slow, true), Outcome::Apply);
        assert_eq!(judge(IMMEDIATE_LIMIT, false), Outcome::Apply);
    }

    #[test]
    fn unchanged_ignores_the_trailing_newline_the_editor_adds() {
        assert!(unchanged("hello", "hello"));
        assert!(unchanged("hello\n", "hello"));
        assert!(unchanged("hello", "hello\r\n"));
        assert!(!unchanged("hello", "hello!"));
        assert!(!unchanged("", "x"));
    }

    #[cfg(unix)]
    #[test]
    fn run_on_file_reports_how_long_the_editor_ran() {
        let dir = temp_dir("elapsed");
        let path = write_draft(&dir, "x").unwrap();
        let command: Vec<String> = ["sh", "-c", "sleep 0.1", "sh"].iter().map(|s| s.to_string()).collect();
        let elapsed = run_on_file(&command, &path).unwrap();
        assert!(elapsed >= Duration::from_millis(100) && elapsed < IMMEDIATE_LIMIT, "{elapsed:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn draft_round_trip_strips_one_newline_and_deletes_the_file() {
        let dir = temp_dir("roundtrip").join("nested");
        let path = write_draft(&dir, "hello\n\n").unwrap();
        assert_eq!(path, dir.join("compose.md"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n\n");
        assert_eq!(read_back(&path).unwrap(), "hello\n");
        assert!(!path.exists());
        let path = write_draft(&dir, "x").unwrap();
        assert_eq!(read_back(&path).unwrap(), "x");
        let path = write_draft(&dir, "crlf\r\n").unwrap();
        assert_eq!(read_back(&path).unwrap(), "crlf");
        assert!(read_back(&path).is_err());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn draft_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("private");
        let path = write_draft(&dir, "secret").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&path), 0o600);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_draft(&dir, "again").unwrap();
        assert_eq!(mode(&path), 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_fake_editor_rewrites_the_file() {
        let dir = temp_dir("fake");
        let path = write_draft(&dir, "old text").unwrap();
        let command: Vec<String> = ["sh", "-c", "printf \"new text\\n\" > \"$1\"", "sh"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        run_on_file(&command, &path).unwrap();
        assert_eq!(read_back(&path).unwrap(), "new text");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn editor_failures_are_reported() {
        let dir = temp_dir("fail");
        let path = write_draft(&dir, "keep").unwrap();
        let failing: Vec<String> = ["sh", "-c", "exit 3", "sh"].iter().map(|s| s.to_string()).collect();
        let err = run_on_file(&failing, &path).unwrap_err();
        assert!(err.contains("exited with"), "{err}");
        let missing = vec!["gramtty-no-such-editor".to_owned()];
        let err = run_on_file(&missing, &path).unwrap_err();
        assert!(err.contains("cannot start"), "{err}");
        assert!(run_on_file(&[], &path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
