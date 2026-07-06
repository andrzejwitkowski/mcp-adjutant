use std::path::Path;
use std::process::Command;

pub fn run_ripgrep(pattern: &str) -> Result<String, String> {
    let output = Command::new("rg")
        .arg(pattern)
        .arg("--max-columns")
        .arg("150")
        .arg("--context")
        .arg("2")
        .output()
        .map_err(|err| format!("failed to spawn ripgrep: {err}"))?;

    if !output.status.success() && output.status.code() != Some(1) {
        return Err(format!(
            "ripgrep failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn run_fd(pattern: &str) -> Result<Vec<String>, String> {
    // ponytail: Debian ships `fdfind`; try fd then fdfind before giving up
    let candidates = ["fd", "fdfind"];
    let mut last_err = String::from("no fd binary found");

    for binary in candidates {
        let output = match Command::new(binary)
            .arg(pattern)
            .arg("-t")
            .arg("f")
            .output()
        {
            Ok(output) => output,
            Err(err) => {
                last_err = format!("failed to spawn {binary}: {err}");
                continue;
            }
        };

        if !output.status.success() {
            last_err = format!(
                "{binary} failed with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            continue;
        }

        let files = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();

        return Ok(files);
    }

    Err(last_err)
}

pub fn read_file_range(path: &Path, start: usize, end: usize) -> Result<String, String> {
    if start == 0 {
        return Err("start must be >= 1 (1-based line numbers)".to_string());
    }
    if start > end {
        return Err(format!("start ({start}) must be <= end ({end})"));
    }

    let content = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;

    let selected = content
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_no = index + 1;
            (line_no >= start && line_no <= end).then_some(line)
        })
        .collect::<Vec<_>>()
        .join("\n");

    if selected.is_empty() && start > content.lines().count() {
        return Ok(String::new());
    }

    Ok(if selected.is_empty() {
        selected
    } else {
        format!("{selected}\n")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/scout")
            .join(name)
    }

    #[test]
    fn read_file_range_smoke() {
        let content = read_file_range(&fixture("readme.txt"), 1, 1).expect("read");
        assert_eq!(content, "alpha marker here\n");
    }
}
