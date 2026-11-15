use std::ops::Range;

use ratatui::style::{Modifier, Style};

use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    Added,
    Removed,
    Meta,
}

struct Spec {
    keywords: &'static [&'static str],
    line_comments: &'static [&'static str],
    block_comments: bool,
    backticks: bool,
    triple: bool,
    single_quotes: bool,
    char_literals: bool,
    case_insensitive: bool,
}

const BASE: Spec = Spec {
    keywords: &[],
    line_comments: &[],
    block_comments: false,
    backticks: false,
    triple: false,
    single_quotes: true,
    char_literals: false,
    case_insensitive: false,
};

const RUST: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
    "use", "where", "while",
];
const PYTHON: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True", "try", "while",
    "with", "yield",
];
const JS: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete",
    "do", "else", "export", "extends", "false", "finally", "for", "from", "function", "if",
    "import", "in", "instanceof", "interface", "let", "new", "null", "of", "return", "static",
    "super", "switch", "this", "throw", "true", "try", "type", "typeof", "undefined", "var",
    "void", "while", "yield",
];
const GO: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "false",
    "for", "func", "go", "goto", "if", "import", "interface", "map", "nil", "package", "range",
    "return", "select", "struct", "switch", "true", "type", "var",
];
const C: &[&str] = &[
    "auto", "break", "case", "catch", "char", "class", "const", "continue", "default", "delete",
    "do", "double", "else", "enum", "extern", "false", "float", "for", "goto", "if", "inline",
    "int", "long", "namespace", "new", "nullptr", "private", "public", "return", "short",
    "signed", "sizeof", "static", "struct", "switch", "template", "this", "throw", "true", "try",
    "typedef", "union", "unsigned", "using", "virtual", "void", "volatile", "while",
];
const JAVA: &[&str] = &[
    "abstract", "boolean", "break", "byte", "case", "catch", "char", "class", "const", "continue",
    "default", "do", "double", "else", "enum", "extends", "false", "final", "finally", "float",
    "for", "if", "implements", "import", "instanceof", "int", "interface", "long", "new", "null",
    "package", "private", "protected", "public", "return", "short", "static", "super", "switch",
    "this", "throw", "throws", "true", "try", "void", "while",
];
const KOTLIN: &[&str] = &[
    "as", "break", "class", "continue", "do", "else", "false", "for", "fun", "if", "in",
    "interface", "is", "null", "object", "override", "package", "private", "public", "return",
    "super", "this", "throw", "true", "try", "typealias", "val", "var", "when", "while",
];
const BASH: &[&str] = &[
    "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in",
    "local", "return", "select", "then", "until", "while",
];
const JSON: &[&str] = &["true", "false", "null"];
const TOML: &[&str] = &["true", "false"];
const YAML: &[&str] = &["true", "false", "null", "yes", "no"];
const SQL: &[&str] = &[
    "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create",
    "table", "drop", "alter", "join", "left", "right", "inner", "outer", "on", "group", "by",
    "order", "having", "limit", "and", "or", "not", "null", "as", "distinct", "union", "index",
    "primary", "key", "foreign", "references", "default", "case", "when", "then", "else", "end",
    "in", "is", "like", "between", "exists", "desc", "asc",
];

fn spec(lang: &str) -> Option<Spec> {
    let slash = &["//"][..];
    let hash = &["#"][..];
    Some(match lang {
        "rust" | "rs" => Spec {
            keywords: RUST,
            line_comments: slash,
            block_comments: true,
            char_literals: true,
            ..BASE
        },
        "python" | "py" => Spec {
            keywords: PYTHON,
            line_comments: hash,
            triple: true,
            ..BASE
        },
        "javascript" | "typescript" | "js" | "ts" | "jsx" | "tsx" => Spec {
            keywords: JS,
            line_comments: slash,
            block_comments: true,
            backticks: true,
            ..BASE
        },
        "go" => Spec {
            keywords: GO,
            line_comments: slash,
            block_comments: true,
            backticks: true,
            ..BASE
        },
        "c" | "cpp" | "h" | "cc" | "c++" => Spec {
            keywords: C,
            line_comments: slash,
            block_comments: true,
            ..BASE
        },
        "java" => Spec {
            keywords: JAVA,
            line_comments: slash,
            block_comments: true,
            ..BASE
        },
        "kotlin" => Spec {
            keywords: KOTLIN,
            line_comments: slash,
            block_comments: true,
            ..BASE
        },
        "bash" | "shell" | "sh" | "zsh" | "fish" => Spec {
            keywords: BASH,
            line_comments: hash,
            ..BASE
        },
        "json" => Spec {
            keywords: JSON,
            single_quotes: false,
            ..BASE
        },
        "toml" => Spec {
            keywords: TOML,
            line_comments: hash,
            ..BASE
        },
        "yaml" | "yml" => Spec {
            keywords: YAML,
            line_comments: hash,
            ..BASE
        },
        "sql" => Spec {
            keywords: SQL,
            line_comments: &["--"],
            block_comments: true,
            case_insensitive: true,
            ..BASE
        },
        _ => return None,
    })
}

pub fn highlight(lang: &str, code: &str) -> Vec<(Range<usize>, Token)> {
    let lang = lang.trim().to_lowercase();
    let chars: Vec<char> = code.chars().collect();
    if lang == "diff" || lang == "patch" {
        return diff(&chars);
    }
    match spec(&lang) {
        Some(spec) => scan(&spec, &chars),
        None => Vec::new(),
    }
}

fn starts_with(chars: &[char], at: usize, pat: &str) -> bool {
    (at..)
        .zip(pat.chars())
        .all(|(i, p)| chars.get(i) == Some(&p))
}

fn line_end(chars: &[char], from: usize) -> usize {
    let mut i = from;
    while i < chars.len() && chars[i] != '\n' {
        i += 1;
    }
    i
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn diff(chars: &[char]) -> Vec<(Range<usize>, Token)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = line_end(chars, start);
        let kind = if starts_with(chars, start, "+++")
            || starts_with(chars, start, "---")
            || starts_with(chars, start, "@@")
            || starts_with(chars, start, "diff ")
            || starts_with(chars, start, "index ")
        {
            Some(Token::Meta)
        } else if chars[start] == '+' {
            Some(Token::Added)
        } else if chars[start] == '-' {
            Some(Token::Removed)
        } else {
            None
        };
        if let Some(kind) = kind
            && end > start
        {
            out.push((start..end, kind));
        }
        start = end + 1;
    }
    out
}

fn string_end(spec: &Spec, chars: &[char], start: usize) -> usize {
    let q = chars[start];
    if spec.triple && (q == '"' || q == '\'') {
        let triple: String = std::iter::repeat_n(q, 3).collect();
        if starts_with(chars, start, &triple) {
            let mut i = start + 3;
            while i < chars.len() {
                if chars[i] == '\\' {
                    i += 2;
                    continue;
                }
                if starts_with(chars, i, &triple) {
                    return i + 3;
                }
                i += 1;
            }
            return chars.len();
        }
    }
    let multiline = q == '`';
    let mut i = start + 1;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && chars.get(i + 1).is_some_and(|n| *n != '\n' && *n != '\r') {
            i += 2;
            continue;
        }
        if c == q {
            return i + 1;
        }
        if !multiline && (c == '\n' || c == '\r') {
            return i;
        }
        i += 1;
    }
    chars.len()
}

fn char_literal_end(chars: &[char], start: usize) -> Option<usize> {
    if chars.get(start + 1) == Some(&'\\') {
        let limit = (start + 12).min(chars.len());
        return (start + 3..limit)
            .find(|&i| chars[i] == '\'')
            .map(|i| i + 1);
    }
    if chars.get(start + 2) == Some(&'\'') && chars.get(start + 1) != Some(&'\n') {
        return Some(start + 3);
    }
    None
}

fn number_end(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() {
        let c = chars[i];
        let keep = c.is_ascii_alphanumeric()
            || c == '_'
            || (c == '.' && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit()));
        if !keep {
            break;
        }
        i += 1;
    }
    i
}

fn scan(spec: &Spec, chars: &[char]) -> Vec<(Range<usize>, Token)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if spec.block_comments && starts_with(chars, i, "/*") {
            let mut j = i + 2;
            while j < chars.len() && !starts_with(chars, j, "*/") {
                j += 1;
            }
            let end = (j + 2).min(chars.len());
            out.push((i..end, Token::Comment));
            i = end;
            continue;
        }
        let comment = spec.line_comments.iter().any(|p| {
            starts_with(chars, i, p) && (*p != "#" || i == 0 || chars[i - 1].is_whitespace())
        });
        if comment {
            let end = line_end(chars, i);
            out.push((i..end, Token::Comment));
            i = end;
            continue;
        }
        let is_string = c == '"'
            || (c == '\'' && spec.single_quotes)
            || (c == '`' && spec.backticks);
        if is_string {
            let end = if c == '\'' && spec.char_literals {
                char_literal_end(chars, i)
            } else {
                Some(string_end(spec, chars, i))
            };
            match end {
                Some(end) => {
                    out.push((i..end, Token::String));
                    i = end;
                }
                None => i += 1,
            }
            continue;
        }
        if c.is_ascii_digit() {
            let end = number_end(chars, i);
            out.push((i..end, Token::Number));
            i = end;
            continue;
        }
        if is_ident_start(c) {
            let mut end = i;
            while end < chars.len() && is_ident(chars[end]) {
                end += 1;
            }
            let word: String = chars[i..end].iter().collect();
            let hit = if spec.case_insensitive {
                let lower = word.to_lowercase();
                spec.keywords.contains(&lower.as_str())
            } else {
                spec.keywords.contains(&word.as_str())
            };
            if hit {
                out.push((i..end, Token::Keyword));
            }
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

pub fn token_style(token: Token) -> Style {
    let coloured = |s: Style, fallback: Modifier| {
        if s.fg.is_some() {
            s
        } else {
            Style::default().add_modifier(fallback)
        }
    };
    match token {
        Token::Plain => Style::default(),
        Token::Keyword => theme::active_field(),
        Token::String => coloured(theme::online(), Modifier::UNDERLINED),
        Token::Comment => theme::muted(),
        Token::Number => coloured(theme::warning(), Modifier::BOLD | Modifier::UNDERLINED),
        Token::Added => theme::online(),
        Token::Removed => theme::error(),
        Token::Meta => coloured(theme::mention(), Modifier::BOLD | Modifier::DIM),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: &str, code: &str) -> Vec<(String, Token)> {
        let chars: Vec<char> = code.chars().collect();
        highlight(lang, code)
            .into_iter()
            .map(|(r, t)| (chars[r].iter().collect(), t))
            .collect()
    }

    #[test]
    fn rust_snippet() {
        let got = kinds("rust", "fn main() { let x = 42; // hi\n let s = \"a\\\"b\"; }");
        assert!(got.contains(&("fn".to_owned(), Token::Keyword)));
        assert!(got.contains(&("let".to_owned(), Token::Keyword)));
        assert!(got.contains(&("42".to_owned(), Token::Number)));
        assert!(got.contains(&("// hi".to_owned(), Token::Comment)));
        assert!(got.contains(&("\"a\\\"b\"".to_owned(), Token::String)));
        assert!(!got.iter().any(|(s, _)| s == "main" || s == "x"));
    }

    #[test]
    fn rust_ranges_are_char_positions() {
        let r = highlight("rs", "é fn");
        assert_eq!(r, vec![(2..4, Token::Keyword)]);
    }

    #[test]
    fn rust_lifetimes_are_not_strings() {
        let got = kinds("rust", "fn f<'a>(x: &'a str) -> char { 'z' }");
        let strings: Vec<_> = got.iter().filter(|(_, t)| *t == Token::String).collect();
        assert_eq!(strings.len(), 1);
        assert_eq!(strings[0].0, "'z'");
    }

    #[test]
    fn keywords_match_whole_words() {
        let got = kinds("rust", "format fnord in_");
        assert!(got.is_empty());
    }

    #[test]
    fn python_triple_quotes() {
        let got = kinds("py", "x = \"\"\"a\nb\"\"\"\n# c\n1.5");
        assert!(got.contains(&("\"\"\"a\nb\"\"\"".to_owned(), Token::String)));
        assert!(got.contains(&("# c".to_owned(), Token::Comment)));
        assert!(got.contains(&("1.5".to_owned(), Token::Number)));
    }

    #[test]
    fn block_comments_and_hex() {
        let got = kinds("c", "/* a\nb */ int x = 0xFF;");
        assert!(got.contains(&("/* a\nb */".to_owned(), Token::Comment)));
        assert!(got.contains(&("int".to_owned(), Token::Keyword)));
        assert!(got.contains(&("0xFF".to_owned(), Token::Number)));
    }

    #[test]
    fn sql_is_case_insensitive() {
        let got = kinds("SQL", "SELECT a FROM t -- x");
        assert!(got.contains(&("SELECT".to_owned(), Token::Keyword)));
        assert!(got.contains(&("FROM".to_owned(), Token::Keyword)));
        assert!(got.contains(&("-- x".to_owned(), Token::Comment)));
    }

    #[test]
    fn hash_comment_needs_a_boundary() {
        let got = kinds("bash", "echo $#\necho a # c");
        assert_eq!(got, vec![("# c".to_owned(), Token::Comment)]);
    }

    #[test]
    fn diff_lines() {
        let got = kinds(
            "diff",
            "diff --git a b\n--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n same",
        );
        let toks: Vec<Token> = got.iter().map(|(_, t)| *t).collect();
        assert_eq!(
            toks,
            vec![
                Token::Meta,
                Token::Meta,
                Token::Meta,
                Token::Meta,
                Token::Removed,
                Token::Added
            ]
        );
        assert_eq!(got[4].0, "-old");
    }

    #[test]
    fn unknown_language_has_no_tokens() {
        assert!(highlight("brainfuck", "fn let 1").is_empty());
        assert!(highlight("", "fn let 1").is_empty());
    }

    #[test]
    fn odd_input_never_panics() {
        let langs = [
            "rust", "py", "js", "go", "c", "java", "kotlin", "bash", "json", "toml", "yaml",
            "sql", "diff",
        ];
        let inputs = [
            "",
            "\"",
            "'",
            "`",
            "\"\"\"",
            "'''",
            "\"'`\"'`",
            "/*",
            "/",
            "\\",
            "'\\",
            "\"\\",
            "😀 fn 😀 \"😀",
            "a\r\nb \"c\r\n// d\r\n",
            "+",
            "-",
            "@@",
            "0x",
            "1.",
            ".5e",
        ];
        for lang in langs {
            for input in inputs {
                let n = input.chars().count();
                for (r, _) in highlight(lang, input) {
                    assert!(r.start < r.end && r.end <= n, "{lang} {input:?} {r:?}");
                }
            }
        }
    }

    #[test]
    fn mono_tokens_differ_by_modifiers() {
        theme::install(std::rc::Rc::new(theme::Theme::preset(
            crate::config::ThemeChoice::Mono,
        )));
        let all = [
            Token::Keyword,
            Token::String,
            Token::Comment,
            Token::Number,
            Token::Meta,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(token_style(*a), token_style(*b), "{a:?} {b:?}");
            }
        }
        theme::install(std::rc::Rc::new(theme::Theme::default()));
    }
}
