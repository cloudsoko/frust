use std::env;
use std::path::PathBuf;

use frust_app_sdk::{check_project, create_project, pack_project, Error};

const HELP: &str = r#"Frust application authoring tools

Usage:
  frust-app new <name> [--dir <path>] [--label <label>]
  frust-app check [path]
  frust-app pack [path] [--output <file>]

Commands:
  new    Create a Rust hook app pinned to the canonical Frust WIT contract
  check  Validate the manifest, ownership, WIT contract and Wasm components
  pack   Produce a deterministic .frust bundle and SHA-256 digest
"#;

fn main() {
    if let Err(error) = run(env::args().skip(1).collect()) {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        print!("{HELP}");
        return Ok(());
    };
    if matches!(command, "help" | "--help" | "-h") {
        print!("{HELP}");
        return Ok(());
    }
    match command {
        "new" => new_command(&args[1..]),
        "check" => check_command(&args[1..]),
        "pack" => pack_command(&args[1..]),
        other => Err(format!("unknown command '{other}'\n\n{HELP}")),
    }
}

fn new_command(args: &[String]) -> Result<(), String> {
    let name = args
        .first()
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| "new requires an app name".to_string())?;
    let dir = option(args, "--dir")?
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(name));
    let label = option(args, "--label")?;
    reject_unknown(args, 1, &["--dir", "--label"])?;
    create_project(&dir, name, label.as_deref()).map_err(display_error)?;
    println!("created {}", dir.display());
    println!("next: cd {} && frust-app check", dir.display());
    Ok(())
}

fn check_command(args: &[String]) -> Result<(), String> {
    if args.len() > 1 {
        return Err("check accepts at most one project path".to_string());
    }
    let root = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let checked = check_project(&root).map_err(display_error)?;
    println!(
        "ok: {} v{} ({} doctypes, {} components, WIT sha256 {})",
        checked.manifest.name,
        checked.manifest.version,
        checked.manifest.doctypes.len(),
        checked.components.len(),
        checked.wit_sha256
    );
    Ok(())
}

fn pack_command(args: &[String]) -> Result<(), String> {
    let root = args
        .first()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let checked = check_project(&root).map_err(display_error)?;
    let output = option(args, "--output")?
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            root.join("dist").join(format!(
                "{}-{}.frust",
                checked.manifest.name, checked.manifest.version
            ))
        });
    reject_unknown(
        args,
        usize::from(args.first().is_some_and(|v| !v.starts_with('-'))),
        &["--output"],
    )?;
    let report = pack_project(&root, &output).map_err(display_error)?;
    println!(
        "packed {} files into {}",
        report.files,
        report.output.display()
    );
    println!("sha256 {}", report.sha256);
    Ok(())
}

fn option(args: &[String], flag: &str) -> Result<Option<String>, String> {
    let Some(index) = args.iter().position(|arg| arg == flag) else {
        return Ok(None);
    };
    args.get(index + 1)
        .filter(|value| !value.starts_with('-'))
        .cloned()
        .map(Some)
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn reject_unknown(args: &[String], positionals: usize, known: &[&str]) -> Result<(), String> {
    let mut index = positionals;
    while index < args.len() {
        let arg = &args[index];
        if known.contains(&arg.as_str()) {
            index += 2;
        } else {
            return Err(format!("unknown argument '{arg}'"));
        }
    }
    Ok(())
}

fn display_error(error: Error) -> String {
    error.to_string()
}
