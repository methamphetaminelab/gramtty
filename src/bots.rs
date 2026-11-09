#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ButtonAction {
    Callback(String),
    Url(String),
    User(i64),
    CopyText(String),
    Text,
    Unsupported(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Button {
    pub text: String,
    pub action: ButtonAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Keyboard {
    pub inline: bool,
    pub rows: Vec<Vec<Button>>,
    pub placeholder: String,
}

impl Keyboard {
    pub fn buttons(&self) -> Vec<&Button> {
        self.rows.iter().flatten().collect()
    }

    pub fn button(&self, index: usize) -> Option<&Button> {
        self.rows.iter().flatten().nth(index)
    }

    pub fn index_of(&self, row: usize, col: usize) -> Option<usize> {
        if col >= self.rows.get(row)?.len() {
            return None;
        }
        Some(self.rows[..row].iter().map(Vec::len).sum::<usize>() + col)
    }
}

const BUTTON_KEYS: &str = "123456789abcdefghimnoprstuvwxyz";

pub fn button_key(index: usize) -> Option<char> {
    BUTTON_KEYS.chars().nth(index)
}

pub fn key_index(key: char) -> Option<usize> {
    BUTTON_KEYS.chars().position(|c| c == key)
}

pub fn button_label(button: &Button, url_mark: &str) -> String {
    match button.action {
        ButtonAction::Url(_) if !url_mark.is_empty() => format!("{} {url_mark}", button.text),
        ButtonAction::Unsupported(_) => format!("{} ✕", button.text),
        _ => button.text.clone(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub index: usize,
    pub x: usize,
    pub width: usize,
}

const MIN_BUTTON: usize = 5;
const MIN_KEYBOARD: usize = 16;

pub fn keyboard_layout(
    kb: &Keyboard,
    width: usize,
    label_width: impl Fn(usize, &Button) -> usize,
) -> Vec<Vec<Cell>> {
    if width < 3 {
        return Vec::new();
    }
    let indexed: Vec<Vec<(usize, &Button)>> = kb
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.is_empty())
        .map(|(r, row)| {
            row.iter()
                .enumerate()
                .filter_map(|(c, b)| kb.index_of(r, c).map(|i| (i, b)))
                .collect()
        })
        .collect();
    let natural = indexed
        .iter()
        .map(|row| {
            let widest = row
                .iter()
                .map(|(i, b)| label_width(*i, b))
                .max()
                .unwrap_or(0);
            row.len() * (widest + 4) + row.len() - 1
        })
        .max()
        .unwrap_or(0);
    let total = natural.max(MIN_KEYBOARD).min(width);
    let per_line = ((total + 1) / (MIN_BUTTON + 1)).max(1);
    let mut out = Vec::new();
    for row in &indexed {
        for chunk in row.chunks(per_line) {
            let n = chunk.len();
            let avail = total.saturating_sub(n - 1);
            let (base, extra) = (avail / n, avail % n);
            let mut x = 0;
            let mut line = Vec::with_capacity(n);
            for (k, (index, _)) in chunk.iter().enumerate() {
                let w = base + usize::from(k < extra);
                line.push(Cell {
                    index: *index,
                    x,
                    width: w,
                });
                x += w + 1;
            }
            out.push(line);
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub bot: String,
}

pub fn command_query(input: &str) -> Option<&str> {
    let rest = input.strip_prefix('/')?;
    if rest.chars().any(char::is_whitespace) {
        return None;
    }
    Some(rest.split('@').next().unwrap_or(rest))
}

pub fn filter_commands<'a>(commands: &'a [Command], input: &str) -> Vec<&'a Command> {
    let Some(query) = command_query(input) else {
        return Vec::new();
    };
    let q = query.to_lowercase();
    let mut out: Vec<(u8, &Command)> = commands
        .iter()
        .filter_map(|c| {
            let name = c.name.to_lowercase();
            if name.starts_with(&q) {
                Some((0, c))
            } else if !q.is_empty() && name.contains(&q) {
                Some((1, c))
            } else {
                None
            }
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    if out.len() == 1 && out[0].1.name.eq_ignore_ascii_case(query) && input.contains('@') {
        return Vec::new();
    }
    out.into_iter().map(|(_, c)| c).collect()
}

pub fn completion(cmd: &Command, multiple_bots: bool) -> String {
    if multiple_bots && !cmd.bot.is_empty() {
        format!("/{}@{} ", cmd.name, cmd.bot)
    } else {
        format!("/{} ", cmd.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn b(text: &str, action: ButtonAction) -> Button {
        Button {
            text: text.to_owned(),
            action,
        }
    }

    fn kb() -> Keyboard {
        Keyboard {
            inline: true,
            rows: vec![
                vec![
                    b("Yes", ButtonAction::Callback("eQ==".into())),
                    b("No", ButtonAction::Callback("bg==".into())),
                ],
                vec![b("Site", ButtonAction::Url("https://example.org".into()))],
                vec![],
                vec![
                    b("A", ButtonAction::Text),
                    b("B", ButtonAction::Text),
                    b("C", ButtonAction::User(5)),
                ],
            ],
            placeholder: String::new(),
        }
    }

    #[test]
    fn maps_buttons_row_major() {
        let k = kb();
        assert_eq!(k.buttons().len(), 6);
        assert_eq!(k.button(0).unwrap().text, "Yes");
        assert_eq!(k.button(2).unwrap().text, "Site");
        assert_eq!(k.button(5).unwrap().action, ButtonAction::User(5));
        assert!(k.button(6).is_none());
        assert_eq!(k.index_of(1, 0), Some(2));
        assert_eq!(k.index_of(3, 2), Some(5));
        assert_eq!(k.index_of(2, 0), None);
        assert_eq!(k.index_of(9, 0), None);
        assert_eq!(button_key(0), Some('1'));
        assert_eq!(button_key(9), Some('a'));
        assert_eq!(key_index('a'), Some(9));
        assert_eq!(key_index('j'), None);
        assert_eq!(key_index('q'), None);
        for i in 0..BUTTON_KEYS.len() {
            assert_eq!(key_index(button_key(i).unwrap()), Some(i));
        }
    }

    #[test]
    fn lays_buttons_out_as_even_boxes_within_width() {
        let k = kb();
        let w = |_: usize, b: &Button| button_label(b, "↗").width();
        let rows = keyboard_layout(&k, 40, w);
        assert_eq!(rows.len(), 3);
        let total = |row: &[Cell]| row.last().map_or(0, |c| c.x + c.width);
        let widest = rows.iter().map(|r| total(r)).max().unwrap();
        assert!((16..=40).contains(&widest), "{rows:?}");
        for row in &rows {
            assert_eq!(total(row), widest, "rows share one width {rows:?}");
            let ws: Vec<usize> = row.iter().map(|c| c.width).collect();
            assert!(
                ws.iter().max().unwrap() - ws.iter().min().unwrap() <= 1,
                "{ws:?}"
            );
            for pair in row.windows(2) {
                assert_eq!(pair[1].x, pair[0].x + pair[0].width + 1);
            }
        }
        assert_eq!(
            rows.iter().flatten().map(|c| c.index).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
        assert_eq!(button_label(k.button(2).unwrap(), "↗"), "Site ↗");
        assert_eq!(button_label(k.button(0).unwrap(), "↗"), "Yes");
        let one = Keyboard {
            inline: true,
            rows: vec![vec![b(
                "Участвовать",
                ButtonAction::Url("https://x".into()),
            )]],
            placeholder: String::new(),
        };
        let r = keyboard_layout(&one, 70, w);
        assert_eq!(
            r[0][0].width,
            "Участвовать ↗".width() + 4,
            "sized to the label"
        );
        let narrow = keyboard_layout(&k, 11, w);
        assert!(
            narrow
                .iter()
                .flatten()
                .all(|c| c.x + c.width <= 11 && c.width >= 5)
        );
        assert!(narrow.len() > 3, "wraps buttons that do not fit");
        assert!(keyboard_layout(&k, 2, w).is_empty());
    }

    #[test]
    fn filters_commands_by_prefix_then_substring() {
        let c = |n: &str, bot: &str| Command {
            name: n.to_owned(),
            description: format!("{n} it"),
            bot: bot.to_owned(),
        };
        let cmds = vec![
            c("start", "abot"),
            c("help", "abot"),
            c("settings", "abot"),
            c("restart", "bbot"),
        ];
        let names = |input: &str| -> Vec<String> {
            filter_commands(&cmds, input)
                .iter()
                .map(|c| c.name.clone())
                .collect()
        };
        assert_eq!(names("/"), ["help", "restart", "settings", "start"]);
        assert_eq!(names("/st"), ["start", "restart"]);
        assert_eq!(names("/ST"), ["start", "restart"]);
        assert_eq!(names("/se"), ["settings"]);
        assert_eq!(names("/he@abot"), ["help"]);
        assert!(names("/help@abot").is_empty());
        assert!(names("/start now").is_empty());
        assert!(names("hello").is_empty());
        assert!(names("/zzz").is_empty());
        assert_eq!(completion(&cmds[0], false), "/start ");
        assert_eq!(completion(&cmds[0], true), "/start@abot ");
        assert_eq!(command_query("/a@b"), Some("a"));
        assert_eq!(command_query("x"), None);
    }
}
