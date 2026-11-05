fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Source/dependency edits can relink the binary without editing this build script.
    // Watch project inputs and small Git identity files, rather than the whole .git tree.
    for path in [
        "src/",
        "Cargo.toml",
        "Cargo.lock",
        "../../Cargo.toml",
        "../../Cargo.lock",
        "../../.cargo/",
    ] {
        if std::path::Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    watch_dependency_sources();
    println!("cargo:rerun-if-env-changed=PLEOCHROMA_BUILD_REVISION");
    println!("cargo:rerun-if-env-changed=PLEOCHROMA_BUILD_DIRTY");
    build_provenance();
    watch_git_identity();

    #[cfg(feature = "gui")]
    compile_ui();
}

fn watch_dependency_sources() {
    let Ok(entries) = std::fs::read_dir("../../crates") else {
        return;
    };
    for entry in entries.flatten() {
        for input in ["src", "Cargo.toml", "Cargo.lock", "build.rs"] {
            let path = entry.path().join(input);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}

fn watch_git_identity() {
    // Git resolves common directories and worktree-specific HEAD/index paths for us.
    let mut names = vec![
        "HEAD".to_owned(),
        "index".to_owned(),
        "packed-refs".to_owned(),
    ];
    if let Some(reference) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        names.push(reference);
    }
    for name in names {
        if let Some(path) = git(&["rev-parse", "--git-path", &name])
            && std::path::Path::new(&path).is_file()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

fn build_provenance() {
    let revision = std::env::var("PLEOCHROMA_BUILD_REVISION")
        .ok()
        .filter(|revision| valid_revision(revision))
        .or_else(|| git(&["rev-parse", "HEAD"]).filter(|revision| valid_revision(revision)))
        .unwrap_or_else(|| "unknown".to_owned());
    let dirty = std::env::var("PLEOCHROMA_BUILD_DIRTY")
        .ok()
        .filter(|value| matches!(value.as_str(), "true" | "false" | "unknown"))
        .or_else(|| {
            git(&["status", "--porcelain", "--untracked-files=normal"])
                .map(|status| (!status.is_empty()).to_string())
        })
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=PLEOCHROMA_BUILD_REVISION={revision}");
    println!("cargo:rustc-env=PLEOCHROMA_BUILD_DIRTY={dirty}");
}

fn valid_revision(revision: &str) -> bool {
    matches!(revision.len(), 40 | 64) && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn git(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(std::env::var_os("CARGO_MANIFEST_DIR")?)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(feature = "gui")]
fn compile_ui() {
    // Slint tracks the files it imports, but an explicit directory watch keeps edits to
    // theme/model files from being missed.
    println!("cargo:rerun-if-changed=ui/");

    // Same style as apps/pleochroma-cut.
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent-dark".to_string()),
    )
    .expect("failed to compile the trainer Slint UI (ui/app.slint)");
}
