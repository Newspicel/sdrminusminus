#!/usr/bin/env python3

import argparse
import re
import subprocess
import sys
from pathlib import Path

RUST_EXT = {".rs"}
JS_EXT = {".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"}
CSS_EXT = {".css"}
ALL_EXT = RUST_EXT | JS_EXT | CSS_EXT

SKIP_DIRS = {"target", "node_modules", ".git", "dist", "build", ".venv"}
SKIP_PATH_PARTS = {"generated"}

KEEP = re.compile(
    r"""
      SPDX-License-Identifier
    | Copyright\s*(?:\(c\)|©|\d{4})
    | Licen[sc]ed\s+under\s+the
    | @license | @preserve
    | @generated | DO\s+NOT\s+EDIT
    | ^\s*(?://[/!]|\*)\s*\#\s*Safety\b
    """,
    re.X | re.I | re.M,
)

DIRECTIVE = re.compile(
    r"""
      eslint-disable | eslint-enable
    | oxlint-disable | oxlint-enable
    | biome-ignore
    | prettier-ignore
    | @ts-ignore | @ts-expect-error | @ts-nocheck
    | istanbul\s+ignore | c8\s+ignore | v8\s+ignore
    | webpackChunkName | webpackIgnore | vite-ignore
    | @vitest-environment | @jsxImportSource
    | ///\s*<reference
    | rustfmt::skip
    """,
    re.X | re.I,
)

SAFETY = re.compile(r"^\s*(?://|/\*)[!/*]?\s*SAFETY\b")


class Scanner:
    def __init__(self, src):
        self.src = src
        self.n = len(src)
        self.i = 0
        self.comments = []

    def add(self, start, end):
        self.comments.append((start, end))

    def line_comment(self):
        j = self.src.find("\n", self.i)
        if j == -1:
            j = self.n
        self.add(self.i, j)
        self.i = j

    def quoted(self, quote):
        i = self.i + 1
        while i < self.n:
            c = self.src[i]
            if c == "\\":
                i += 2
                continue
            if c == quote:
                i += 1
                break
            if c == "\n" and quote != "`":
                break
            i += 1
        self.i = i


def scan_rust(src):
    s = Scanner(src)
    ident = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
    while s.i < s.n:
        c = src[s.i]
        if c == "/" and s.i + 1 < s.n:
            nxt = src[s.i + 1]
            if nxt == "/":
                if s.i > 0 and src[s.i - 1] == ":":
                    s.i += 2
                    continue
                s.line_comment()
                continue
            if nxt == "*":
                depth, j = 1, s.i + 2
                while j < s.n and depth:
                    if src.startswith("/*", j):
                        depth += 1
                        j += 2
                    elif src.startswith("*/", j):
                        depth -= 1
                        j += 2
                    else:
                        j += 1
                s.add(s.i, j)
                s.i = j
                continue
            s.i += 1
            continue
        if c == '"':
            s.quoted('"')
            continue
        if c == "'":
            if s.i + 1 < s.n and src[s.i + 1] == "\\":
                s.quoted("'")
            elif s.i + 2 < s.n and src[s.i + 2] == "'":
                s.i += 3
            else:
                s.i += 1
            continue
        m = ident.match(src, s.i)
        if m:
            word = m.group(0)
            j = m.end()
            if word in {"r", "br", "rb", "cr", "rc"} and j < s.n and src[j] in '#"':
                hashes = 0
                while j < s.n and src[j] == "#":
                    hashes += 1
                    j += 1
                if j < s.n and src[j] == '"':
                    close = '"' + "#" * hashes
                    k = src.find(close, j + 1)
                    s.i = s.n if k == -1 else k + len(close)
                    continue
            if word in {"b", "c"} and j < s.n and src[j] == '"':
                s.i = j
                s.quoted('"')
                continue
            s.i = j
            continue
        s.i += 1
    return s.comments


JS_REGEX_PREV = re.compile(r"[({\[,;:=!&|?+\-*%~^<>]$")
JS_REGEX_KEYWORD = re.compile(
    r"\b(return|typeof|instanceof|in|of|new|delete|void|throw|case|do|else|yield|await)$"
)


def scan_js(src):
    s = Scanner(src)
    ident = re.compile(r"[A-Za-z_$][A-Za-z0-9_$]*")
    template_stack = []
    prev = ""

    def regex_allowed():
        p = prev.rstrip()
        if not p:
            return True
        return bool(JS_REGEX_PREV.search(p)) or bool(JS_REGEX_KEYWORD.search(p))

    while s.i < s.n:
        c = src[s.i]
        if c == "/" and s.i + 1 < s.n:
            nxt = src[s.i + 1]
            if nxt == "/":
                if s.i > 0 and src[s.i - 1] == ":":
                    s.i += 2
                    continue
                start = s.i
                s.line_comment()
                prev = src[start:s.i]
                continue
            if nxt == "*":
                j = src.find("*/", s.i + 2)
                j = s.n if j == -1 else j + 2
                s.add(s.i, j)
                s.i = j
                continue
            if regex_allowed():
                j = s.i + 1
                in_class = False
                while j < s.n:
                    ch = src[j]
                    if ch == "\\":
                        j += 2
                        continue
                    if ch == "\n":
                        break
                    if ch == "[":
                        in_class = True
                    elif ch == "]":
                        in_class = False
                    elif ch == "/" and not in_class:
                        j += 1
                        break
                    j += 1
                s.i = j
                prev = "/"
                continue
            s.i += 1
            prev = "/"
            continue
        if c in "\"'":
            s.quoted(c)
            prev = "s"
            continue
        if c == "`":
            i = s.i + 1
            while i < s.n:
                ch = src[i]
                if ch == "\\":
                    i += 2
                    continue
                if ch == "`":
                    i += 1
                    break
                if ch == "$" and i + 1 < s.n and src[i + 1] == "{":
                    template_stack.append(0)
                    i += 2
                    break
                i += 1
            s.i = i
            prev = "`"
            continue
        if template_stack:
            if c == "{":
                template_stack[-1] += 1
            elif c == "}":
                if template_stack[-1] == 0:
                    template_stack.pop()
                    i = s.i + 1
                    while i < s.n:
                        ch = src[i]
                        if ch == "\\":
                            i += 2
                            continue
                        if ch == "`":
                            i += 1
                            break
                        if ch == "$" and i + 1 < s.n and src[i + 1] == "{":
                            template_stack.append(0)
                            i += 2
                            break
                        i += 1
                    s.i = i
                    prev = "`"
                    continue
                template_stack[-1] -= 1
        m = ident.match(src, s.i)
        if m:
            prev = m.group(0)
            s.i = m.end()
            continue
        if not c.isspace():
            prev = c
        s.i += 1
    return s.comments


def scan_css(src):
    s = Scanner(src)
    while s.i < s.n:
        c = src[s.i]
        if c == "/" and s.i + 1 < s.n and src[s.i + 1] == "*":
            j = src.find("*/", s.i + 2)
            j = s.n if j == -1 else j + 2
            s.add(s.i, j)
            s.i = j
            continue
        if c in "\"'":
            s.quoted(c)
            continue
        s.i += 1
    return s.comments


def merge_line_comments(src, comments):
    merged = []
    for start, end in comments:
        if src[start:start + 2] != "//" or not merged:
            merged.append([start, end])
            continue
        prev_start, prev_end = merged[-1]
        gap = src[prev_end:start]
        if src[prev_start:prev_start + 2] == "//" and gap.count("\n") == 1 and not gap.strip():
            merged[-1][1] = end
            continue
        merged.append([start, end])
    return [(start, end) for start, end in merged]


def scanner_for(path):
    ext = path.suffix
    if ext in RUST_EXT:
        return scan_rust
    if ext in JS_EXT:
        return scan_js
    if ext in CSS_EXT:
        return scan_css
    return None


def widen_jsx_container(src, start, end):
    left = start
    while left > 0 and src[left - 1] in " \t":
        left -= 1
    if left == 0 or src[left - 1] != "{":
        return start, end
    brace = left - 1
    line_start = src.rfind("\n", 0, brace) + 1
    if src[line_start:brace].strip():
        return start, end
    right = end
    while right < len(src) and src[right] in " \t\n":
        right += 1
    if right >= len(src) or src[right] != "}":
        return start, end
    return brace, right + 1


def strip(src, comments, keep_safety, jsx):
    out = []
    cursor = 0
    removed = 0
    for start, end in comments:
        text = src[start:end]
        if KEEP.search(text) or (keep_safety and SAFETY.search(text)):
            continue
        if DIRECTIVE.search(text):
            offset = 0
            for line in text.splitlines(keepends=True):
                if DIRECTIVE.search(line):
                    break
                offset += len(line)
            if offset == 0:
                continue
            end = start + offset
            text = src[start:end]
        if jsx and text.startswith("/*"):
            start, end = widen_jsx_container(src, start, end)
        if start < cursor:
            continue
        out.append(src[cursor:start])
        out.append("\n" * src[start:end].count("\n"))
        cursor = end
        removed += 1
    out.append(src[cursor:])
    return "".join(out), removed


def tidy(original, stripped):
    orig_lines = original.split("\n")
    new_lines = stripped.split("\n")
    kept = []
    for idx, line in enumerate(new_lines):
        if not line.strip():
            was_blank = idx < len(orig_lines) and not orig_lines[idx].strip()
            if not was_blank:
                continue
            if kept and not kept[-1].strip():
                continue
            kept.append("")
            continue
        kept.append(line.rstrip())
    while kept and not kept[0].strip():
        kept.pop(0)
    while kept and not kept[-1].strip():
        kept.pop()
    if not kept:
        return ""
    return "\n".join(kept) + "\n"


def tracked_files(root):
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=root,
        capture_output=True,
        text=True,
        check=True,
    )
    for name in result.stdout.split("\0"):
        if not name:
            continue
        path = Path(name)
        if path.suffix not in ALL_EXT:
            continue
        parts = set(path.parts)
        if parts & SKIP_DIRS or parts & SKIP_PATH_PARTS:
            continue
        yield root / path


def main():
    parser = argparse.ArgumentParser(
        description="Strip comments from tracked Rust, TypeScript, JavaScript and CSS sources."
    )
    parser.add_argument("paths", nargs="*", type=Path, help="files to process (default: all tracked sources)")
    parser.add_argument("--root", type=Path, default=Path.cwd(), help="repository root")
    parser.add_argument("--check", action="store_true", help="report without writing; exit 1 if anything would change")
    parser.add_argument("--no-keep-safety", action="store_true", help="also strip `SAFETY:` comments on unsafe code")
    parser.add_argument("--quiet", action="store_true", help="print only the summary")
    args = parser.parse_args()

    root = args.root.resolve()
    files = [p.resolve() for p in args.paths] if args.paths else list(tracked_files(root))

    changed_files = 0
    total_removed = 0
    for path in files:
        scan = scanner_for(path)
        if scan is None:
            continue
        original = path.read_text(encoding="utf-8")
        comments = merge_line_comments(original, scan(original))
        if not comments:
            continue
        stripped, removed = strip(
            original, comments, not args.no_keep_safety, path.suffix in {".tsx", ".jsx"}
        )
        if not removed:
            continue
        result = tidy(original, stripped)
        if result == original:
            continue
        changed_files += 1
        total_removed += removed
        if not args.quiet:
            rel = path.relative_to(root) if path.is_relative_to(root) else path
            print(f"{rel}: {removed}")
        if not args.check:
            path.write_text(result, encoding="utf-8")

    verb = "would remove" if args.check else "removed"
    print(f"{verb} {total_removed} comments across {changed_files} files")
    return 1 if args.check and changed_files else 0


if __name__ == "__main__":
    sys.exit(main())
