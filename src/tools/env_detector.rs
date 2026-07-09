use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cache::mcp_workspace_root;
use crate::domain::AdjutantConfig;

struct ModuleBoundary {
    marker: &'static str,
    command: &'static str,
}

// ponytail: ordered table of known manifests; niche stacks (CUDA, Bazel, …) use triage_overrides
const MODULE_BOUNDARIES: &[ModuleBoundary] = &[
    ModuleBoundary {
        marker: "Cargo.toml",
        command: "cargo check --message-format=json",
    },
    ModuleBoundary {
        marker: "package.json",
        command: "npm run typecheck",
    },
    ModuleBoundary {
        marker: "pyproject.toml",
        command: "python -m compileall -q .",
    },
    ModuleBoundary {
        marker: "requirements.txt",
        command: "python -m compileall -q .",
    },
    ModuleBoundary {
        marker: "setup.py",
        command: "python -m compileall -q .",
    },
    ModuleBoundary {
        marker: "Pipfile",
        command: "python -m compileall -q .",
    },
    ModuleBoundary {
        marker: "pom.xml",
        command: "mvn -q -DskipTests compile",
    },
    ModuleBoundary {
        marker: "CMakeLists.txt",
        command: "cmake --build build 2>/dev/null || (cmake -S . -B build && cmake --build build)",
    },
    ModuleBoundary {
        marker: "meson.build",
        command:
            "meson compile -C build 2>/dev/null || (meson setup build && meson compile -C build)",
    },
    ModuleBoundary {
        marker: "Makefile",
        command: "make -k",
    },
];

const GRADLE_MARKERS: &[&str] = &[
    "settings.gradle.kts",
    "build.gradle.kts",
    "settings.gradle",
    "build.gradle",
];

pub fn find_nearest_module_boundary(
    start_path: &Path,
    config: &AdjutantConfig,
) -> Option<(PathBuf, String)> {
    let mut current = if start_path.is_file() {
        start_path.parent()?.to_path_buf()
    } else {
        start_path.to_path_buf()
    };

    loop {
        if let Some(cmd) = match_triage_override(&current, config) {
            return Some((current.clone(), cmd));
        }
        if let Some(cmd) = detect_builtin_boundary(&current) {
            return Some((current.clone(), cmd));
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn detect_builtin_boundary(dir: &Path) -> Option<String> {
    if GRADLE_MARKERS
        .iter()
        .any(|marker| dir.join(marker).is_file())
    {
        return Some(gradle_check_command(dir));
    }

    for boundary in MODULE_BOUNDARIES {
        if dir.join(boundary.marker).is_file() {
            return Some(boundary.command.to_string());
        }
    }

    None
}

fn gradle_check_command(dir: &Path) -> String {
    if dir.join("gradlew").is_file() {
        "./gradlew check --no-daemon".to_string()
    } else {
        "gradle check".to_string()
    }
}

fn match_triage_override(dir: &Path, config: &AdjutantConfig) -> Option<String> {
    let overrides = config.triage_overrides.as_ref()?;
    for (prefix, cmd) in overrides {
        let normalized = prefix.trim_end_matches('/');
        if dir.ends_with(normalized) {
            return Some(cmd.clone());
        }
    }
    None
}

pub fn get_dirty_files_from_git() -> Result<Vec<PathBuf>, String> {
    let repo_root = mcp_workspace_root();
    let output = Command::new("git")
        .current_dir(&repo_root)
        .args(["status", "--porcelain"])
        .output()
        .map_err(|err| format!("failed to spawn git: {err}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();

    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        if !status
            .chars()
            .any(|c| matches!(c, 'M' | 'A' | '?' | 'R' | 'T'))
        {
            continue;
        }

        let mut path_part = line[3..].trim();
        if let Some(arrow) = path_part.rfind(" -> ") {
            path_part = &path_part[arrow + 4..];
        }
        path_part = path_part.trim_matches('"');
        if !path_part.is_empty() {
            files.push(repo_root.join(path_part));
        }
    }

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("mcp-adjutant-{test_name}-{nanos}"))
    }

    #[test]
    fn find_nearest_module_boundary_prefers_override() {
        let root = temp_root("override");
        let frontend = root.join("monorepo/frontend");
        fs::create_dir_all(frontend.join("src")).expect("dirs");
        fs::write(frontend.join("package.json"), "{}").expect("package.json");
        fs::write(frontend.join("src/App.tsx"), "export {}").expect("app");

        let config = AdjutantConfig {
            triage_overrides: Some(HashMap::from([(
                "frontend/".to_string(),
                "npm run build".to_string(),
            )])),
            ..Default::default()
        };

        let (dir, cmd) =
            find_nearest_module_boundary(&frontend.join("src/App.tsx"), &config).expect("boundary");
        assert_eq!(dir, frontend);
        assert_eq!(cmd, "npm run build");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn triage_override_does_not_match_substring_of_directory_name() {
        let root = temp_root("override-substring");
        let backend = root.join("monorepo/backend");
        fs::create_dir_all(backend.join("src")).expect("dirs");
        fs::write(
            backend.join("Cargo.toml"),
            "[package]\nname = \"backend\"\n",
        )
        .expect("cargo");

        let config = AdjutantConfig {
            triage_overrides: Some(HashMap::from([(
                "end/".to_string(),
                "npm run build".to_string(),
            )])),
            ..Default::default()
        };

        let (dir, cmd) =
            find_nearest_module_boundary(&backend.join("src/lib.rs"), &config).expect("boundary");
        assert_eq!(dir, backend);
        assert_eq!(cmd, "cargo check --message-format=json");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn find_nearest_module_boundary_detects_common_ecosystems() {
        let config = AdjutantConfig::default();

        let root = temp_root("python");
        fs::create_dir_all(root.join("service")).expect("dirs");
        fs::write(
            root.join("service/pyproject.toml"),
            "[project]\nname = \"svc\"\n",
        )
        .expect("pyproject");
        let (dir, cmd) = find_nearest_module_boundary(&root.join("service/app.py"), &config)
            .expect("python boundary");
        assert_eq!(dir, root.join("service"));
        assert_eq!(cmd, "python -m compileall -q .");
        fs::remove_dir_all(&root).ok();

        let root = temp_root("java");
        fs::create_dir_all(root.join("api/src")).expect("dirs");
        fs::write(root.join("api/pom.xml"), "<project></project>").expect("pom");
        let (dir, cmd) = find_nearest_module_boundary(&root.join("api/src/Main.java"), &config)
            .expect("java boundary");
        assert_eq!(dir, root.join("api"));
        assert_eq!(cmd, "mvn -q -DskipTests compile");
        fs::remove_dir_all(&root).ok();

        let root = temp_root("kotlin");
        fs::create_dir_all(root.join("mobile")).expect("dirs");
        fs::write(
            root.join("mobile/settings.gradle.kts"),
            "rootProject.name = \"app\"",
        )
        .expect("gradle");
        let (dir, cmd) = find_nearest_module_boundary(&root.join("mobile/App.kt"), &config)
            .expect("kotlin boundary");
        assert_eq!(dir, root.join("mobile"));
        assert_eq!(cmd, "gradle check");
        fs::remove_dir_all(&root).ok();

        let root = temp_root("cpp");
        fs::create_dir_all(root.join("native/src")).expect("dirs");
        fs::write(
            root.join("native/CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.0)",
        )
        .expect("cmake");
        let (dir, cmd) = find_nearest_module_boundary(&root.join("native/src/main.cpp"), &config)
            .expect("cpp boundary");
        assert_eq!(dir, root.join("native"));
        assert!(cmd.contains("cmake --build build"));
        fs::remove_dir_all(&root).ok();

        let root = temp_root("makefile");
        fs::create_dir_all(root.join("firmware")).expect("dirs");
        fs::write(root.join("firmware/Makefile"), "all:\n\ttrue\n").expect("makefile");
        let (dir, cmd) = find_nearest_module_boundary(&root.join("firmware/device.c"), &config)
            .expect("c boundary");
        assert_eq!(dir, root.join("firmware"));
        assert_eq!(cmd, "make -k");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn triage_override_covers_niche_stacks_like_cuda() {
        let root = temp_root("cuda-override");
        let cuda = root.join("kernels");
        fs::create_dir_all(&cuda).expect("dirs");
        fs::write(cuda.join("kernel.cu"), "__global__ void k() {}").expect("cu");

        let config = AdjutantConfig {
            triage_overrides: Some(HashMap::from([(
                "kernels/".to_string(),
                "nvcc -std=c++17 -c kernel.cu".to_string(),
            )])),
            ..Default::default()
        };

        let (dir, cmd) =
            find_nearest_module_boundary(&cuda.join("kernel.cu"), &config).expect("cuda boundary");
        assert_eq!(dir, cuda);
        assert_eq!(cmd, "nvcc -std=c++17 -c kernel.cu");

        fs::remove_dir_all(&root).ok();
    }
}
