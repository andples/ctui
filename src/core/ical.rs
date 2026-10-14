//! A small iCalendar (RFC 5545) reader and writer that keeps everything.
//!
//! Files are parsed into a tree of [`Component`]s holding raw [`Prop`]s. ctui
//! reads and changes only the properties it understands; every other
//! property, parameter and component is written back exactly as it came in,
//! so editing an item never loses another client's data.

use std::fmt::Write as _;

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    /// Upper-cased property name (`SUMMARY`, `X-FOO`).
    pub name: String,
    /// Parameters in file order; names upper-cased, values unquoted.
    pub params: Vec<(String, String)>,
    /// The raw value, still escaped as in the file.
    pub value: String,
}

impl Prop {
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            value: value.into(),
        }
    }

    pub fn with_param(mut self, name: &str, value: &str) -> Self {
        self.params
            .push((name.to_ascii_uppercase(), value.to_string()));
        self
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The value as text, with `\n`, `\,`, `\;` and `\\` unescaped.
    pub fn text(&self) -> String {
        unescape(&self.value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Component {
    /// Upper-cased (`VCALENDAR`, `VEVENT`).
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            ..Self::default()
        }
    }

    pub fn get(&self, name: &str) -> Option<&Prop> {
        self.props
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Prop> + 'a {
        self.props
            .iter()
            .filter(move |p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn text(&self, name: &str) -> Option<String> {
        self.get(name).map(Prop::text)
    }

    /// Replaces the first property with this name (keeping its position) and
    /// drops any others; appends when there is none.
    pub fn set(&mut self, prop: Prop) {
        if let Some(i) = self.props.iter().position(|p| p.name == prop.name) {
            self.props[i] = prop;
            let name = self.props[i].name.clone();
            let mut seen = false;
            self.props.retain(|p| {
                if p.name != name {
                    return true;
                }
                let keep = !seen;
                seen = true;
                keep
            });
        } else {
            self.props.push(prop);
        }
    }

    pub fn set_text(&mut self, name: &str, text: &str) {
        self.set(Prop::new(name, escape(text)));
    }

    pub fn remove(&mut self, name: &str) {
        self.props.retain(|p| !p.name.eq_ignore_ascii_case(name));
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Component> + 'a {
        self.children
            .iter()
            .filter(move |c| c.name.eq_ignore_ascii_case(name))
    }
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

pub fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Joins folded lines (a line starting with a space or tab continues the
/// previous one) and drops blank lines.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = raw.strip_prefix([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        if !raw.trim().is_empty() {
            lines.push(raw.to_string());
        }
    }
    lines
}

fn parse_line(line: &str) -> Result<Prop> {
    // name *(";" param) ":" value — quoted parameter values may hold ; and :
    let mut in_quotes = false;
    let mut split = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes => {
                split = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(colon) = split else {
        bail!("no ':' in line {line:?}")
    };
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = Vec::new();
    let mut cur = String::new();
    in_quotes = false;
    for c in head.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ';' if !in_quotes => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    let name = parts.remove(0).trim().to_ascii_uppercase();
    if name.is_empty() {
        bail!("empty property name in {line:?}");
    }
    let params = parts
        .into_iter()
        .filter_map(|p| {
            p.split_once('=')
                .map(|(n, v)| (n.trim().to_ascii_uppercase(), v.to_string()))
        })
        .collect();
    Ok(Prop {
        name,
        params,
        value: value.to_string(),
    })
}

/// Parses every top-level component in `text` (usually one `VCALENDAR`).
pub fn parse(text: &str) -> Result<Vec<Component>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut stack: Vec<Component> = Vec::new();
    let mut top = Vec::new();
    for line in unfold(text) {
        let prop = match parse_line(&line) {
            Ok(p) => p,
            // Tolerate junk lines inside a component rather than refusing the file.
            Err(_) if !stack.is_empty() => continue,
            Err(e) => return Err(e),
        };
        match prop.name.as_str() {
            "BEGIN" => stack.push(Component::new(prop.value.trim())),
            "END" => {
                let Some(done) = stack.pop() else {
                    bail!("END:{} without BEGIN", prop.value)
                };
                if !done.name.eq_ignore_ascii_case(prop.value.trim()) {
                    bail!("END:{} closes BEGIN:{}", prop.value, done.name);
                }
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => top.push(done),
                }
            }
            _ => match stack.last_mut() {
                Some(c) => c.props.push(prop),
                None => bail!("property {} outside any component", prop.name),
            },
        }
    }
    if let Some(open) = stack.last() {
        bail!("BEGIN:{} is never closed", open.name);
    }
    Ok(top)
}

fn needs_quotes(v: &str) -> bool {
    v.contains([':', ';', ','])
}

fn write_prop(out: &mut String, p: &Prop) {
    let mut line = p.name.clone();
    for (n, v) in &p.params {
        if needs_quotes(v) && !v.starts_with('"') {
            let _ = write!(line, ";{n}=\"{v}\"");
        } else {
            let _ = write!(line, ";{n}={v}");
        }
    }
    line.push(':');
    line.push_str(&p.value);
    fold_into(out, &line);
}

/// Folds at 75 octets without splitting a UTF-8 character.
fn fold_into(out: &mut String, line: &str) {
    let mut width = 0;
    for c in line.chars() {
        let len = c.len_utf8();
        if width + len > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += len;
    }
    out.push_str("\r\n");
}

pub fn write(component: &Component) -> String {
    let mut out = String::new();
    write_into(&mut out, component);
    out
}

fn write_into(out: &mut String, c: &Component) {
    let _ = write!(out, "BEGIN:{}\r\n", c.name);
    for p in &c.props {
        write_prop(out, p);
    }
    for child in &c.children {
        write_into(out, child);
    }
    let _ = write!(out, "END:{}\r\n", c.name);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\nX-WR-CALNAME:Work\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Europe/London:20261015T093000\r\nDTEND;TZID=Europe/London:20261015T100000\r\nSUMMARY:Stand\\, up\\; daily\r\nDESCRIPTION:a long line that goes on and on and on and on and on and on and on a\r\n nd keeps going\r\nATTENDEE;CN=\"Doe, Jane\";ROLE=REQ-PARTICIPANT:mailto:jane@example.com\r\nX-UNKNOWN;X-P=1:keep me\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn parses_and_unfolds() {
        let cals = parse(SAMPLE).unwrap();
        assert_eq!(cals.len(), 1);
        let ev = &cals[0].children[0];
        assert_eq!(ev.name, "VEVENT");
        assert_eq!(ev.text("SUMMARY").unwrap(), "Stand, up; daily");
        assert!(ev.text("DESCRIPTION").unwrap().ends_with("and keeps going"));
        assert_eq!(
            ev.get("DTSTART").unwrap().param("tzid"),
            Some("Europe/London")
        );
        let att = ev.get("ATTENDEE").unwrap();
        assert_eq!(att.param("CN"), Some("Doe, Jane"));
        assert_eq!(att.value, "mailto:jane@example.com");
        assert_eq!(ev.children[0].get("TRIGGER").unwrap().value, "-PT10M");
    }

    #[test]
    fn round_trips_everything() {
        let first = parse(SAMPLE).unwrap();
        let written = write(&first[0]);
        let again = parse(&written).unwrap();
        assert_eq!(first, again);
        assert!(written.lines().all(|l| l.len() <= 76));
    }

    #[test]
    fn folds_multibyte_safely() {
        let mut c = Component::new("VTODO");
        c.set_text("SUMMARY", &"é".repeat(100));
        let text = write(&c);
        let back = parse(&text).unwrap();
        assert_eq!(back[0].text("SUMMARY").unwrap(), "é".repeat(100));
    }

    #[test]
    fn set_replaces_in_place() {
        let mut c = Component::new("VTODO");
        c.props.push(Prop::new("SUMMARY", "a"));
        c.props.push(Prop::new("X-A", "1"));
        c.props.push(Prop::new("SUMMARY", "b"));
        c.set_text("SUMMARY", "c");
        assert_eq!(c.props.len(), 2);
        assert_eq!(c.props[0].value, "c");
    }

    #[test]
    fn escapes() {
        let s = "a, b; c\\d\nnext";
        assert_eq!(unescape(&escape(s)), s);
    }

    #[test]
    fn rejects_unbalanced() {
        assert!(parse("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VCALENDAR\r\n").is_err());
    }
}
