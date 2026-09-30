use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(crate) const MAX_LINES: usize = 3_000;
const EM_DASH: char = '\u{2014}';
const SKIPPED_DIRS: [&str; 4] = [
    "Core/Generated",
    "Core/Artifacts",
    "SDRmm.xcodeproj",
    "build",
];
const TEXT_EXTENSIONS: [&str; 7] = [
    "yml",
    "xcconfig",
    "plist",
    "entitlements",
    "xcprivacy",
    "json",
    "xcstrings",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FindingKind {
    Comment,
    EmDash,
    TooLong(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
    pub(crate) kind: FindingKind,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.kind {
            FindingKind::Comment => "comment".to_owned(),
            FindingKind::EmDash => "em dash".to_owned(),
            FindingKind::TooLong(lines) => format!("{lines} lines, limit {MAX_LINES}"),
        };
        write!(f, "{}:{}: {what}", self.path.display(), self.line)
    }
}

pub(crate) fn scan_tree(root: &Path) -> Result<Vec<Finding>> {
    let app = root.join("apps/ios");
    let mut findings = Vec::new();
    if app.is_dir() {
        walk(&app, &app, &mut findings)?;
    }
    Ok(findings)
}

fn walk(app: &Path, dir: &Path, findings: &mut Vec<Finding>) -> Result<()> {
    let mut entries = std::fs::read_dir(dir)
        .with_context(|| format!("read {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("list {}", dir.display()))?;
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if skipped(app, &path) {
            continue;
        }
        if path.is_dir() {
            walk(app, &path, findings)?;
        } else {
            scan_file(&path, findings)?;
        }
    }
    Ok(())
}

fn skipped(app: &Path, path: &Path) -> bool {
    path.strip_prefix(app)
        .is_ok_and(|relative| SKIPPED_DIRS.iter().any(|dir| relative == Path::new(dir)))
}

fn scan_file(path: &Path, findings: &mut Vec<Finding>) -> Result<()> {
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let swift = extension == "swift";
    if !swift && !TEXT_EXTENSIONS.contains(&extension) {
        return Ok(());
    }
    let source =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if swift {
        findings.extend(scan_swift(path, &source));
    } else {
        findings.extend(scan_text(path, &source));
    }
    Ok(())
}

pub(crate) fn scan_text(path: &Path, source: &str) -> Vec<Finding> {
    let style = TextStyle::of(path);
    let mut findings = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let number = index + 1;
        if line.contains(EM_DASH) {
            findings.push(finding(path, number, FindingKind::EmDash));
        }
        if style.is_comment(line) {
            findings.push(finding(path, number, FindingKind::Comment));
        }
    }
    findings
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextStyle {
    Yaml,
    XcConfig,
    Xml,
    Plain,
}

impl TextStyle {
    fn of(path: &Path) -> Self {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("yml") => Self::Yaml,
            Some("xcconfig") => Self::XcConfig,
            Some("plist" | "entitlements" | "xcprivacy") => Self::Xml,
            _ => Self::Plain,
        }
    }

    fn is_comment(self, line: &str) -> bool {
        match self {
            Self::Yaml => yaml_comment(line),
            Self::XcConfig => line.contains("//"),
            Self::Xml => line.contains("<!--"),
            Self::Plain => false,
        }
    }
}

fn yaml_comment(line: &str) -> bool {
    let mut quote = None;
    let mut previous = ' ';
    for c in line.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '#') if previous.is_whitespace() => return true,
            _ => {}
        }
        previous = c;
    }
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Literal {
    Plain,
    Multiline,
    Raw(usize),
    RawMultiline(usize),
}

impl Literal {
    const fn hashes(self) -> usize {
        match self {
            Self::Plain | Self::Multiline => 0,
            Self::Raw(hashes) | Self::RawMultiline(hashes) => hashes,
        }
    }

    const fn multiline(self) -> bool {
        matches!(self, Self::Multiline | Self::RawMultiline(_))
    }

    const fn quotes(self) -> usize {
        if self.multiline() { 3 } else { 1 }
    }
}

struct Lexer<'a> {
    chars: Vec<char>,
    at: usize,
    line: usize,
    path: &'a Path,
    findings: Vec<Finding>,
    literals: Vec<(Literal, usize)>,
    in_literal: Option<Literal>,
}

pub(crate) fn scan_swift(path: &Path, source: &str) -> Vec<Finding> {
    let mut lexer = Lexer {
        chars: source.chars().collect(),
        at: 0,
        line: 1,
        path,
        findings: Vec::new(),
        literals: Vec::new(),
        in_literal: None,
    };
    lexer.run();
    let mut findings = lexer.findings;
    for (index, line) in source.lines().enumerate() {
        if line.contains(EM_DASH) {
            findings.push(finding(path, index + 1, FindingKind::EmDash));
        }
    }
    let lines = source.lines().count();
    if lines > MAX_LINES {
        findings.push(finding(path, lines, FindingKind::TooLong(lines)));
    }
    findings.sort_by_key(|found| found.line);
    findings
}

impl Lexer<'_> {
    fn run(&mut self) {
        while self.at < self.chars.len() {
            match self.in_literal {
                Some(literal) => self.literal_step(literal),
                None => self.code_step(),
            }
        }
    }

    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.at + offset).copied()
    }

    fn advance(&mut self, count: usize) {
        for _ in 0..count {
            if self.peek(0) == Some('\n') {
                self.line += 1;
            }
            self.at += 1;
        }
    }

    fn repeated(&self, from: usize, c: char) -> usize {
        self.chars[self.at + from..]
            .iter()
            .take_while(|&&next| next == c)
            .count()
    }

    fn code_step(&mut self) {
        match (self.peek(0), self.peek(1)) {
            (Some('/'), Some('/' | '*')) => self.comment(),
            (Some('#'), _) => self.maybe_raw_literal(),
            (Some('"'), _) => self.open(0),
            (Some('('), _) => {
                if let Some((_, depth)) = self.literals.last_mut() {
                    *depth += 1;
                }
                self.advance(1);
            }
            (Some(')'), _) => self.close_paren(),
            _ => self.advance(1),
        }
    }

    fn comment(&mut self) {
        self.findings
            .push(finding(self.path, self.line, FindingKind::Comment));
        if self.peek(1) == Some('/') {
            while self.peek(0).is_some_and(|c| c != '\n') {
                self.advance(1);
            }
            return;
        }
        self.advance(2);
        while self.at < self.chars.len()
            && !(self.peek(0) == Some('*') && self.peek(1) == Some('/'))
        {
            self.advance(1);
        }
        self.advance(2);
    }

    fn maybe_raw_literal(&mut self) {
        let hashes = self.repeated(0, '#');
        if self.peek(hashes) == Some('"') {
            self.advance(hashes);
            self.open(hashes);
        } else {
            self.advance(hashes);
        }
    }

    fn open(&mut self, hashes: usize) {
        let multiline = self.repeated(0, '"') >= 3;
        let literal = match (hashes, multiline) {
            (0, false) => Literal::Plain,
            (0, true) => Literal::Multiline,
            (count, false) => Literal::Raw(count),
            (count, true) => Literal::RawMultiline(count),
        };
        self.advance(literal.quotes());
        self.in_literal = Some(literal);
    }

    fn close_paren(&mut self) {
        self.advance(1);
        let Some((literal, depth)) = self.literals.last_mut() else {
            return;
        };
        if *depth > 1 {
            *depth -= 1;
            return;
        }
        let literal = *literal;
        self.literals.pop();
        self.in_literal = Some(literal);
    }

    fn literal_step(&mut self, literal: Literal) {
        let hashes = literal.hashes();
        match self.peek(0) {
            Some('\\') if self.repeated(1, '#') >= hashes => {
                if self.peek(1 + hashes) == Some('(') {
                    self.advance(2 + hashes);
                    self.literals.push((literal, 1));
                    self.in_literal = None;
                } else {
                    self.advance(2 + hashes);
                }
            }
            Some('"') if self.closes(literal) => {
                self.advance(literal.quotes() + hashes);
                self.in_literal = None;
            }
            _ => self.advance(1),
        }
    }

    fn closes(&self, literal: Literal) -> bool {
        let quotes = literal.quotes();
        self.repeated(0, '"') >= quotes && self.repeated(quotes, '#') >= literal.hashes()
    }
}

fn finding(path: &Path, line: usize, kind: FindingKind) -> Finding {
    Finding {
        path: path.to_path_buf(),
        line,
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<(usize, FindingKind)> {
        scan_swift(Path::new("A.swift"), source)
            .into_iter()
            .map(|found| (found.line, found.kind))
            .collect()
    }

    #[test]
    fn finds_line_comment() {
        assert_eq!(
            kinds("let a = 1\nlet b = 2 // two\n"),
            vec![(2, FindingKind::Comment)]
        );
    }

    #[test]
    fn finds_block_comment() {
        assert_eq!(
            kinds("let a = 1\n/* one\n two */\nlet b = 2\n"),
            vec![(2, FindingKind::Comment)]
        );
    }

    #[test]
    fn ignores_slashes_in_string() {
        assert!(kinds("let url = \"https://example.com/*x*/\"\n").is_empty());
    }

    #[test]
    fn ignores_url_in_multiline_string() {
        let source = "let text = \"\"\"\n  see https://example.com\n  \"quoted\" // not code\n  \"\"\"\nlet b = 1\n";
        assert!(kinds(source).is_empty());
    }

    #[test]
    fn handles_raw_string_with_hashes() {
        let source = "let a = #\"a \"quote\" // b \\(no)\"#\nlet b = ##\"\"\"\n\"# //\n\"\"\"##\nlet c = 1 // yes\n";
        assert_eq!(kinds(source), vec![(5, FindingKind::Comment)]);
    }

    #[test]
    fn handles_interpolation_with_nested_string() {
        let source = "let a = \"x \\(f(\"y // z\", (1)) + g()) //w\" // real\n";
        assert_eq!(kinds(source), vec![(1, FindingKind::Comment)]);
        let comment_in_interpolation = "let a = \"\\(b /* c */)\"\n";
        assert_eq!(
            kinds(comment_in_interpolation),
            vec![(1, FindingKind::Comment)]
        );
    }

    #[test]
    fn raw_interpolation_needs_its_hashes() {
        let source = "let a = #\"\\(x // y)\"#\nlet b = #\"\\#(x) // z\"#\n";
        assert!(kinds(source).is_empty());
    }

    #[test]
    fn finds_em_dash_in_literal() {
        assert_eq!(
            kinds("let a = \"a \u{2014} b\"\n"),
            vec![(1, FindingKind::EmDash)]
        );
    }

    #[test]
    fn flags_file_over_limit() {
        let source = "let a = 1\n".repeat(MAX_LINES + 1);
        assert_eq!(
            kinds(&source),
            vec![(MAX_LINES + 1, FindingKind::TooLong(MAX_LINES + 1))]
        );
        assert!(kinds(&"let a = 1\n".repeat(MAX_LINES)).is_empty());
    }

    #[test]
    fn text_files_flag_their_comment_styles() {
        let yml = scan_text(
            Path::new("project.yml"),
            "name: SDRmm\n# note\nkey: \"a # b\" # tail\n",
        );
        assert_eq!(
            yml.iter().map(|found| found.line).collect::<Vec<_>>(),
            vec![2, 3]
        );
        let xcconfig = scan_text(
            Path::new("Shared.xcconfig"),
            "#include \"Local.xcconfig\"\n// note\nA = B\n",
        );
        assert_eq!(xcconfig.len(), 1);
        assert_eq!(xcconfig[0].line, 2);
        let plist = scan_text(Path::new("Info.plist"), "<dict>\n<!-- x -->\n</dict>\n");
        assert_eq!(plist.len(), 1);
        let privacy = scan_text(Path::new("PrivacyInfo.xcprivacy"), "<!-- x -->\n");
        assert_eq!(privacy.len(), 1);
        let json = scan_text(Path::new("a.json"), "{\"a\": \"b \u{2014} c\"}\n");
        assert_eq!(json[0].kind, FindingKind::EmDash);
    }

    #[test]
    fn scan_tree_skips_generated_dirs() {
        let dir = std::env::temp_dir().join(format!("sdrmm-ios-scan-{}", std::process::id()));
        let generated = dir.join("apps/ios/Core/Generated");
        let sources = dir.join("apps/ios/App");
        std::fs::create_dir_all(&generated).expect("generated dir");
        std::fs::create_dir_all(&sources).expect("sources dir");
        std::fs::write(generated.join("SdrmmCore.swift"), "// generated\n").expect("write");
        std::fs::write(sources.join("A.swift"), "let a = 1 // no\n").expect("write");
        std::fs::write(sources.join("notes.txt"), "// ignored\n").expect("write");
        let found = scan_tree(&dir).expect("scanned");
        std::fs::remove_dir_all(&dir).expect("cleaned");
        assert_eq!(found.len(), 1);
        assert!(found[0].path.ends_with("App/A.swift"));
    }
}
