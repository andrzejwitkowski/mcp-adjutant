use std::path::Path;
use std::process::Command;

pub fn run_build_command(dir: &Path, command: &str) -> Result<String, String> {
    // ponytail: sh -c keeps one spawn path for arbitrary compiler commands
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .output()
        .map_err(|err| format!("failed to spawn build command: {err}"))?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if output.status.success() {
        Ok(combined)
    } else {
        Err(combined)
    }
}

pub fn edit_file_line(path: &Path, line_number: usize, new_content: &str) -> Result<(), String> {
    if line_number == 0 {
        return Err("line_number must be >= 1".to_string());
    }

    let content = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let had_trailing_newline = content.ends_with('\n');
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();

    if line_number > lines.len() {
        return Err(format!(
            "line {line_number} out of range (file has {} lines)",
            lines.len()
        ));
    }

    lines[line_number - 1] = new_content.to_string();
    let mut updated = lines.join("\n");
    if had_trailing_newline {
        updated.push('\n');
    }

    std::fs::write(path, updated)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn edit_file_line_replaces_target_line() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mcp-adjutant-edit-{nanos}.txt"));
        fs::write(&path, "alpha\nbeta\ngamma\n").expect("write");

        edit_file_line(&path, 2, "bravo").expect("edit");
        let updated = fs::read_to_string(&path).expect("read");
        assert_eq!(updated, "alpha\nbravo\ngamma\n");

        fs::remove_file(&path).ok();
    }
}
