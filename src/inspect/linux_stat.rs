//! Pure /proc parser: fixtures also run on macOS.
use anyhow::{ensure, Context, Result};

use super::Identity;

pub(super) fn parse_stat(bytes: &[u8]) -> Result<Identity> {
    // comm may contain spaces, newlines, and ')' itself. The final ')' closes comm.
    let close = bytes
        .iter()
        .rposition(|b| *b == b')')
        .context("malformed process stat")?;
    let open = bytes
        .iter()
        .position(|b| *b == b'(')
        .context("malformed process stat")?;
    ensure!(open > 0 && close > open, "malformed process stat");
    let pid = std::str::from_utf8(&bytes[..open])?.trim().parse::<u32>()?;
    let fields: Vec<_> = std::str::from_utf8(&bytes[close + 1..])?
        .split_ascii_whitespace()
        .collect();
    ensure!(fields.len() >= 20, "truncated process stat");
    let foreground = fields[5].parse::<i32>()?;
    let tty = fields[4].parse::<i32>()? as u32;
    Ok(Identity {
        pid,
        parent: fields[1].parse()?,
        group: fields[2].parse()?,
        tty: u64::from(tty),
        foreground: if foreground > 0 { foreground as u32 } else { 0 },
        started: (fields[19].parse()?, 0),
        runnable: matches!(fields[0], "R" | "S" | "D" | "I"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_handles_parentheses_newlines_and_non_utf8_comm() {
        let mut stat = b"123 (a ) (\xff\nname))".to_vec();
        stat.extend_from_slice(
            format!(" S 100 123 100 34817 123 {}987 0", "0 ".repeat(13)).as_bytes(),
        );
        let parsed = parse_stat(&stat).unwrap();
        assert_eq!(parsed.pid, 123);
        assert_eq!(parsed.parent, 100);
        assert_eq!(parsed.group, 123);
        assert_eq!(parsed.tty, 34817);
        assert_eq!(parsed.foreground, 123);
        assert_eq!(parsed.started, (987, 0));
        assert!(parsed.runnable);
    }

    #[test]
    fn stat_rejects_truncation_and_stopped_states() {
        assert!(parse_stat(b"1 (bad) S 0").is_err());
        for state in ["T", "t", "Z", "X"] {
            let stat = format!("1 (test) {state} 0 1 1 0 -1 {}9", "0 ".repeat(13));
            let parsed = parse_stat(stat.as_bytes()).unwrap();
            assert!(!parsed.runnable);
            assert_eq!(parsed.foreground, 0);
        }
    }
}
