use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, bail};
use sdrmm_channels::testgen::ident_fixtures::{Expect, FIXTURES, identify, judge, samples};

pub fn run(root: &Path) -> Result<()> {
    let dir = root.join("fixtures");
    let mut confusion: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut misses = Vec::new();
    println!(
        "{:<28} {:<12} {:<12} {:<34} {:>7}  verdict",
        "fixture", "expected", "found", "named", "signals"
    );
    for fixture in FIXTURES {
        let bytes = std::fs::read(dir.join(fixture.file()))
            .with_context(|| format!("read {}", fixture.file()))?;
        let outcome = identify(fixture, &samples(&bytes));
        let verdict = match (&fixture.expect, judge(fixture, &outcome)) {
            (Expect::Beyond(why), _) => format!("beyond reach: {why}"),
            (_, Ok(())) => "ok".to_owned(),
            (_, Err(miss)) => {
                misses.push(miss);
                "MISS".to_owned()
            }
        };
        if verbose() || verdict == "MISS" {
            for line in &outcome.details {
                println!("    {line}");
            }
        }
        let expected = fixture
            .expected_family()
            .map_or("-", |family| family.label());
        *confusion
            .entry((expected.to_owned(), outcome.modulation.label().to_owned()))
            .or_default() += 1;
        println!(
            "{:<28} {:<12} {:<12} {:<34} {:>7}  {}",
            fixture.stem,
            expected,
            outcome.modulation.label(),
            format!(
                "{}{}",
                outcome.name.as_deref().unwrap_or("-"),
                if outcome.confirmed { " ✓" } else { "" }
            ),
            outcome.signals,
            verdict
        );
    }
    print_confusion(&confusion);
    if !misses.is_empty() {
        bail!(
            "{} fixture(s) misidentified:\n{}",
            misses.len(),
            misses.join("\n")
        );
    }
    Ok(())
}

fn verbose() -> bool {
    std::env::var_os("IDENT_MATRIX_VERBOSE").is_some()
}

fn print_confusion(confusion: &BTreeMap<(String, String), usize>) {
    let mut expected: Vec<&str> = confusion.keys().map(|(e, _)| e.as_str()).collect();
    expected.sort_unstable();
    expected.dedup();
    let mut found: Vec<&str> = confusion.keys().map(|(_, f)| f.as_str()).collect();
    found.sort_unstable();
    found.dedup();
    println!();
    print!("{:<20}", "expected \\ found");
    for f in &found {
        print!("{f:>14}");
    }
    println!();
    for e in &expected {
        print!("{e:<20}");
        for f in &found {
            let n = confusion
                .get(&((*e).to_owned(), (*f).to_owned()))
                .copied()
                .unwrap_or(0);
            if n == 0 {
                print!("{:>14}", ".");
            } else {
                print!("{n:>14}");
            }
        }
        println!();
    }
}
