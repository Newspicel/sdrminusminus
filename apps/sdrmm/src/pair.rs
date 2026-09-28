use std::path::PathBuf;

use qrcodegen::{QrCode, QrCodeEcc};
use sdrmm_server::phones::cli::{caption, offer_for_cli};

const QUIET_ZONE: usize = 2;
const DARK_ON_LIGHT: &str = "\x1b[30;47m";
const RESET: &str = "\x1b[0m";

#[derive(clap::Args, Debug)]
pub(crate) struct PairArgs {
    #[arg(long)]
    pub(crate) db: Option<PathBuf>,
    #[arg(long)]
    pub(crate) name: Option<String>,
    #[arg(long)]
    pub(crate) plain: bool,
}

pub(crate) fn run(args: PairArgs) -> anyhow::Result<()> {
    let db = crate::resolve_db_path(args.db)?;
    let offer = offer_for_cli(&db, args.name.as_deref())?;
    let code = QrCode::encode_text(&offer.uri, QrCodeEcc::Medium)?;
    let plain = args.plain || std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    print!("{}", render(&modules(&code), plain));
    println!("{}", caption(&offer));
    Ok(())
}

fn modules(code: &QrCode) -> Vec<Vec<bool>> {
    (0..code.size())
        .map(|y| (0..code.size()).map(|x| code.get_module(x, y)).collect())
        .collect()
}

pub(crate) fn render(modules: &[Vec<bool>], plain: bool) -> String {
    let size = modules.len() + 2 * QUIET_ZONE;
    let dark = |x: usize, y: usize| {
        x.checked_sub(QUIET_ZONE)
            .zip(y.checked_sub(QUIET_ZONE))
            .and_then(|(x, y)| modules.get(y).and_then(|row| row.get(x)))
            .copied()
            .unwrap_or(false)
    };
    let mut out = String::new();
    for y in (0..size).step_by(2) {
        if !plain {
            out.push_str(DARK_ON_LIGHT);
        }
        out.extend((0..size).map(|x| match (dark(x, y), dark(x, y + 1)) {
            (true, true) => '█',
            (true, false) => '▀',
            (false, true) => '▄',
            (false, false) => ' ',
        }));
        if !plain {
            out.push_str(RESET);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix() -> Vec<Vec<bool>> {
        [[1, 0, 0, 1], [0, 1, 1, 0], [1, 1, 0, 0], [1, 0, 1, 1]]
            .iter()
            .map(|row| row.iter().map(|module| *module == 1).collect())
            .collect()
    }

    #[test]
    fn half_blocks_render_two_rows_per_line() {
        let quiet = " ".repeat(8);
        let lines = [
            quiet.clone(),
            "  ▀▄▄▀  ".to_owned(),
            "  █▀▄▄  ".to_owned(),
            quiet,
        ];
        let plain: String = lines.iter().map(|line| format!("{line}\n")).collect();
        assert_eq!(render(&matrix(), true), plain);
        let colored: String = lines
            .iter()
            .map(|line| format!("\x1b[30;47m{line}\x1b[0m\n"))
            .collect();
        assert_eq!(render(&matrix(), false), colored);
    }

    #[test]
    fn an_odd_size_pads_the_last_line_with_light() {
        let rendered = render(&[vec![true]], true);
        assert_eq!(rendered, "     \n  ▀  \n     \n");
    }

    #[test]
    fn a_pair_link_fits_a_qr_code() {
        let link = format!(
            "sdrmm://pair?h={}&c=48210937&fp={}&p=1&n=shack",
            ["192.168.1.20:8443"; 6].join(","),
            "a".repeat(64)
        );
        let code = QrCode::encode_text(&link, QrCodeEcc::Medium).expect("encode");
        let rows = modules(&code);
        assert_eq!(rows.len(), usize::try_from(code.size()).expect("size"));
        assert!(rows.iter().all(|row| row.len() == rows.len()));
    }
}
