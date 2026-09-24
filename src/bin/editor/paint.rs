use std::env;

/// ANSI styling for the preview. Standard colors, so the terminal's own palette
/// (Tokyo Night in `WezTerm`) decides the exact shades. Pad and wrap before painting.
#[derive(Clone, Copy)]
pub(super) struct Paint {
    enabled: bool,
}

impl Paint {
    #[cfg(test)]
    pub(super) const PLAIN: Self = Self { enabled: false };

    /// Colors only a terminal, and honors `NO_COLOR` (<https://no-color.org>).
    pub(super) fn detect(terminal: bool) -> Self {
        let no_color = env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        Self {
            enabled: terminal && !no_color,
        }
    }

    fn style(self, code: &str, text: &str) -> String {
        if self.enabled && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    pub(super) fn bold(self, text: &str) -> String {
        self.style("1", text)
    }

    pub(super) fn dim(self, text: &str) -> String {
        self.style("2", text)
    }

    pub(super) fn red(self, text: &str) -> String {
        self.style("31", text)
    }

    pub(super) fn green(self, text: &str) -> String {
        self.style("32", text)
    }

    pub(super) fn yellow(self, text: &str) -> String {
        self.style("33", text)
    }

    pub(super) fn cyan(self, text: &str) -> String {
        self.style("36", text)
    }

    /// Chosen or applied: bold green.
    pub(super) fn chosen(self, text: &str) -> String {
        self.style("1;32", text)
    }

    /// A section rule: dim dashes around a bold title.
    pub(super) fn rule(self, title: &str) -> String {
        format!("{} {} {}", self.dim("──"), self.bold(title), self.dim("──"))
    }

    /// One payload line: Markdown headings bold blue, prompt tags cyan, and the
    /// standardizer's notes block dimmed so the user's words stand out.
    pub(super) fn payload_line(self, line: &str, in_notes: bool) -> String {
        if !self.enabled {
            return line.to_owned();
        }
        if in_notes {
            return self.dim(line);
        }
        if line.starts_with("## ") {
            return self.style("1;34", line);
        }
        let mut painted = String::with_capacity(line.len());
        let mut rest = line;
        while let Some(start) = rest.find('<') {
            let tail = &rest[start..];
            let length = tag_length(tail).max(1);
            painted.push_str(&rest[..start]);
            let tag = &tail[..length];
            painted.push_str(&if length > 1 {
                self.cyan(tag)
            } else {
                tag.to_owned()
            });
            rest = &tail[length..];
        }
        painted.push_str(rest);
        painted
    }
}

/// Byte length of a leading `<name>` or `</name>` with a lowercase name, else 0.
fn tag_length(text: &str) -> usize {
    let inner = &text[1..];
    let body = inner.strip_prefix('/').unwrap_or(inner);
    let name = body
        .bytes()
        .take_while(|byte| byte.is_ascii_lowercase() || *byte == b'_')
        .count();
    if name > 0 && body.as_bytes().get(name) == Some(&b'>') {
        text.len() - body.len() + name + 1
    } else {
        0
    }
}

#[cfg(test)]
pub(super) const COLOR: Paint = Paint { enabled: true };

#[cfg(test)]
pub(super) fn strip(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("\x1b[") {
        plain.push_str(&rest[..start]);
        let tail = &rest[start..];
        rest = tail.find('m').map_or("", |end| &tail[end + 1..]);
    }
    plain.push_str(rest);
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_cyan_but_comparisons_are_not() {
        let line = COLOR.payload_line("<task>keep a < b && c</task>", false);
        assert_eq!(
            line,
            "\x1b[36m<task>\x1b[0mkeep a < b && c\x1b[36m</task>\x1b[0m"
        );
        assert_eq!(strip(&line), "<task>keep a < b && c</task>");
        assert_eq!(
            Paint::PLAIN.payload_line("<task>x</task>", false),
            "<task>x</task>"
        );
    }
}
