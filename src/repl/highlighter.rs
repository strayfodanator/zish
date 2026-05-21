use crate::config::Config;
use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};

/// Real-time syntax highlighter for the zish command line
pub struct ZishHighlighter {
    cmd_style: Style,
    invalid_cmd_style: Style,
    arg_style: Style,
    string_style: Style,
    comment_style: Style,
    operator_style: Style,
    number_style: Style,
    path_style: Style,
    variable_style: Style,
}

impl ZishHighlighter {
    pub fn new(config: &Config) -> Self {
        Self {
            cmd_style: parse_style(&config.colors.command),
            invalid_cmd_style: parse_style(&config.colors.invalid_command),
            arg_style: parse_style(&config.colors.argument),
            string_style: parse_style(&config.colors.string),
            comment_style: parse_style(&config.colors.comment),
            operator_style: parse_style(&config.colors.operator),
            number_style: parse_style(&config.colors.number),
            path_style: parse_style(&config.colors.path),
            variable_style: parse_style(&config.colors.variable),
        }
    }
}

impl Highlighter for ZishHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();
        let tokens = tokenize_for_highlight(line);
        let mut is_first_word = true;
        let mut prev_was_operator = true; // treat start of line as after operator

        for token in tokens {
            let style = match token.kind {
                HighlightKind::Comment => self.comment_style,

                HighlightKind::Operator => {
                    is_first_word = true;
                    prev_was_operator = true;
                    self.operator_style
                }

                HighlightKind::String => self.string_style,

                HighlightKind::Variable => self.variable_style,

                HighlightKind::Number => self.number_style,

                HighlightKind::Path => self.path_style,

                HighlightKind::Word if prev_was_operator || is_first_word => {
                    is_first_word = false;
                    prev_was_operator = false;
                    // Check if it's a valid command
                    if is_valid_command(&token.text) {
                        self.cmd_style
                    } else {
                        self.invalid_cmd_style
                    }
                }

                HighlightKind::Word => {
                    // Argument
                    if token.text.starts_with('/') || token.text.starts_with("./") || token.text.starts_with("~/") {
                        // Path argument
                        self.path_style
                    } else if token.text.starts_with('-') {
                        // Flag
                        self.arg_style
                    } else {
                        self.arg_style
                    }
                }

                HighlightKind::Whitespace => Style::new(),
            };

            styled.push((style, token.text));
        }

        styled
    }
}

// ─── Tokenizer for highlighting ───────────────────────────────────────────────

#[derive(Debug)]
enum HighlightKind {
    Word,
    String,
    Variable,
    Operator,
    Number,
    Comment,
    Path,
    Whitespace,
}

struct HToken {
    kind: HighlightKind,
    text: String,
}

fn tokenize_for_highlight(line: &str) -> Vec<HToken> {
    let mut tokens = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i] as char;

        match c {
            // Whitespace
            ' ' | '\t' => {
                let start = i;
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                tokens.push(HToken {
                    kind: HighlightKind::Whitespace,
                    text: line[start..i].to_string(),
                });
            }

            // Comment
            '#' => {
                tokens.push(HToken {
                    kind: HighlightKind::Comment,
                    text: line[i..].to_string(),
                });
                break;
            }

            // Operators
            '|' | '&' | ';' | '(' | ')' | '{' | '}' => {
                let start = i;
                i += 1;
                // Multi-char operators
                if i < bytes.len() && matches!((bytes[i-1], bytes[i]), (b'|', b'|') | (b'&', b'&') | (b'|', b'&')) {
                    i += 1;
                }
                tokens.push(HToken {
                    kind: HighlightKind::Operator,
                    text: line[start..i].to_string(),
                });
            }

            // Redirects
            '>' | '<' => {
                let start = i;
                i += 1;
                while i < bytes.len() && matches!(bytes[i], b'>' | b'<' | b'&' | b'0'..=b'9') {
                    i += 1;
                }
                tokens.push(HToken {
                    kind: HighlightKind::Operator,
                    text: line[start..i].to_string(),
                });
            }

            // Single-quoted string
            '\'' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() { i += 1; } // closing '
                tokens.push(HToken {
                    kind: HighlightKind::String,
                    text: line[start..i].to_string(),
                });
            }

            // Double-quoted string
            '"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' { i += 1; } // skip escaped char
                    i += 1;
                }
                if i < bytes.len() { i += 1; } // closing "
                tokens.push(HToken {
                    kind: HighlightKind::String,
                    text: line[start..i].to_string(),
                });
            }

            // Variable
            '$' => {
                let start = i;
                i += 1;
                while i < bytes.len()
                    && ((bytes[i] as char).is_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'{')
                {
                    if bytes[i] == b'{' {
                        while i < bytes.len() && bytes[i] != b'}' { i += 1; }
                        if i < bytes.len() { i += 1; }
                        break;
                    }
                    i += 1;
                }
                tokens.push(HToken {
                    kind: HighlightKind::Variable,
                    text: line[start..i].to_string(),
                });
            }

            // Number (standalone)
            '0'..='9' => {
                let start = i;
                while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
                    i += 1;
                }
                // If followed by non-word char, it's a number; otherwise it's a word
                let is_num = i >= bytes.len() || bytes[i] == b' ' || bytes[i] == b'\t';
                tokens.push(HToken {
                    kind: if is_num { HighlightKind::Number } else { HighlightKind::Word },
                    text: line[start..i].to_string(),
                });
            }

            // Regular word
            _ => {
                let start = i;
                while i < bytes.len() {
                    let b = bytes[i];
                    if matches!(b, b' ' | b'\t' | b'|' | b'&' | b';' | b'(' | b')' | b'{' | b'}' | b'<' | b'>' | b'\'' | b'"' | b'#') {
                        break;
                    }
                    i += 1;
                }
                let text = line[start..i].to_string();
                // Detect path
                let kind = if text.starts_with('/') || text.starts_with("./") || text.starts_with("~/") {
                    HighlightKind::Path
                } else {
                    HighlightKind::Word
                };
                tokens.push(HToken { kind, text });
            }
        }
    }

    tokens
}

/// Check if a command is valid (exists in PATH, is a builtin, alias, or function)
fn is_valid_command(name: &str) -> bool {
    // Builtins
    if matches!(name, "cd" | "pwd" | "echo" | "printf" | "export" | "unset" | "alias" |
        "unalias" | "source" | "." | "exit" | "true" | "false" | ":" | "set" |
        "return" | "break" | "continue" | "type" | "which" | "jobs" | "fg" | "bg" |
        "history" | "test" | "[" | "read" | "eval" | "exec" | "wait" | "function") {
        return true;
    }
    // Keywords
    if matches!(name, "if" | "then" | "else" | "elif" | "fi" | "for" | "while" |
        "until" | "do" | "done" | "case" | "esac" | "in") {
        return true;
    }
    // PATH lookup
    std::env::split_paths(&std::env::var("PATH").unwrap_or_default())
        .any(|dir| dir.join(name).exists())
}

// ─── Style parser ──────────────────────────────────────────────────────────────

pub fn parse_style(s: &str) -> Style {
    let mut style = Style::new();
    let parts: Vec<&str> = s.split_whitespace().collect();
    let mut color_parts = Vec::new();

    for part in &parts {
        match *part {
            "bold" => style = style.bold(),
            "italic" => style = style.italic(),
            "underline" => style = style.underline(),
            "dimmed" => style = style.dimmed(),
            "blink" => style = style.blink(),
            "reverse" => style = style.reverse(),
            _ => color_parts.push(*part),
        }
    }

    if let Some(color) = parse_color(&color_parts.join(" ")) {
        style = style.fg(color);
    }

    style
}

fn parse_color(s: &str) -> Option<Color> {
    match s.to_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" | "purple" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "bright black" | "dark gray" | "darkgray" => Some(Color::DarkGray),
        "bright red" => Some(Color::LightRed),
        "bright green" => Some(Color::LightGreen),
        "bright yellow" => Some(Color::LightYellow),
        "bright blue" => Some(Color::LightBlue),
        "bright magenta" | "bright purple" => Some(Color::LightMagenta),
        "bright cyan" => Some(Color::LightCyan),
        "bright white" => Some(Color::White),
        s if s.starts_with('#') && s.len() == 7 => {
            let r = u8::from_str_radix(&s[1..3], 16).ok()?;
            let g = u8::from_str_radix(&s[3..5], 16).ok()?;
            let b = u8::from_str_radix(&s[5..7], 16).ok()?;
            Some(Color::Rgb(r, g, b))
        }
        s if s.parse::<u8>().is_ok() => {
            Some(Color::Fixed(s.parse().unwrap()))
        }
        _ => None,
    }
}
