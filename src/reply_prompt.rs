use std::process::Stdio;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(300);

pub const TOOL_NAMES: [&str; 5] = ["fuzzel", "rofi", "wofi", "zenity", "kdialog"];
pub const TOOLS: &str = "fuzzel, rofi, wofi, zenity, kdialog";

fn clean_title(title: &str) -> String {
    title.chars().filter(|c| !c.is_control()).collect()
}

pub fn prompt_command(
    configured: &[String],
    title: &str,
    available: impl Fn(&str) -> bool,
) -> Option<Vec<String>> {
    let title = clean_title(title);
    let fill = |args: &[&str]| -> Vec<String> {
        args.iter().map(|a| a.replace("{title}", &title)).collect()
    };
    if !configured.is_empty() {
        let args: Vec<&str> = configured.iter().map(String::as_str).collect();
        return Some(fill(&args));
    }
    let candidates: [&[&str]; 5] = [
        &["fuzzel", "--dmenu", "--prompt-only=Reply to {title}: "],
        &["rofi", "-dmenu", "-p", "Reply to {title}"],
        &["wofi", "--dmenu", "--prompt", "Reply to {title}"],
        &[
            "zenity",
            "--entry",
            "--title",
            "gramtty",
            "--text",
            "Reply to {title}",
        ],
        &["kdialog", "--inputbox", "Reply to {title}"],
    ];
    candidates.iter().find(|c| available(c[0])).map(|c| fill(c))
}

pub async fn run(command: &[String]) -> Result<Option<String>, String> {
    let Some((program, args)) = command.split_first() else {
        return Err("reply prompt command is empty".to_owned());
    };
    let child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("reply prompt {program}: {e}"))?;
    let Ok(result) = tokio::time::timeout(WAIT, child.wait_with_output()).await else {
        return Ok(None);
    };
    let out = result.map_err(|e| format!("reply prompt {program}: {e}"))?;
    if !out.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok(Some(text).filter(|t| !t.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(name: &'static str) -> impl Fn(&str) -> bool {
        move |p| p == name
    }

    #[test]
    fn configured_command_gets_the_title() {
        let configured = ["my-prompt".to_owned(), "--ask={title}".to_owned()];
        assert_eq!(
            prompt_command(&configured, "Ann\u{1b}[31m\n", |_| false),
            Some(vec!["my-prompt".to_owned(), "--ask=Ann[31m".to_owned()])
        );
    }

    #[test]
    fn each_tool_is_detected_with_its_flags() {
        let cmd = |tool: &'static str| prompt_command(&[], "Ann", only(tool)).unwrap();
        assert_eq!(
            cmd("fuzzel"),
            ["fuzzel", "--dmenu", "--prompt-only=Reply to Ann: "]
        );
        assert_eq!(cmd("rofi"), ["rofi", "-dmenu", "-p", "Reply to Ann"]);
        assert_eq!(cmd("wofi"), ["wofi", "--dmenu", "--prompt", "Reply to Ann"]);
        assert_eq!(
            cmd("zenity"),
            [
                "zenity",
                "--entry",
                "--title",
                "gramtty",
                "--text",
                "Reply to Ann"
            ]
        );
        assert_eq!(cmd("kdialog"), ["kdialog", "--inputbox", "Reply to Ann"]);
    }

    #[test]
    fn the_first_available_tool_wins() {
        let cmd = prompt_command(&[], "Ann", |p| p == "zenity" || p == "rofi").unwrap();
        assert_eq!(cmd[0], "rofi");
    }

    #[test]
    fn no_tool_means_no_command() {
        assert_eq!(prompt_command(&[], "Ann", |_| false), None);
    }

    #[tokio::test]
    async fn runner_returns_trimmed_stdout() {
        let echo = ["sh", "-c", "echo hello"].map(str::to_owned);
        assert_eq!(run(&echo).await, Ok(Some("hello".to_owned())));
        let blank = ["sh", "-c", "echo"].map(str::to_owned);
        assert_eq!(run(&blank).await, Ok(None));
        let esc = ["sh", "-c", "echo typed; exit 1"].map(str::to_owned);
        assert_eq!(run(&esc).await, Ok(None));
        let missing = ["gramtty-no-such-prompt".to_owned()];
        assert!(run(&missing).await.is_err());
    }
}
