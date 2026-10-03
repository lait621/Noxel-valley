//! `valley-artgen` — the sprite generator for Noxel Valley.
//!
//! ```text
//! valley-artgen --out DIR [--scale N] [--check]
//! ```
//!
//! Writes `DIR/farm/`: five atlases (terrain, crops, props, characters, ui),
//! their JSON, and a contact sheet of everything at `--scale`.
//!
//! Built on `noxel-gen`'s library half, so the palette, the drawing helpers and
//! the error type are the engine's rather than a copy that drifts. What lives
//! here is the part that is *this game's*: which sprites exist, what they look
//! like, and the region names the game loads them by.
//!
//! Generation is deterministic. Running it twice changes no bytes, so a diff in
//! `assets/` means something real changed.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use noxel_valley_artgen::farm;

const USAGE: &str = "\
valley-artgen — sprite generator for Noxel Valley

USAGE:
    valley-artgen --out DIR [--scale N]
    valley-artgen --out DIR --check

OPTIONS:
    --out DIR    Where to write. The atlases go in <DIR>/farm.
    --scale N    Contact-sheet upscale (default 3).
    --check      Regenerate in memory and compare against the files on disk.
                 Exits non-zero if anything differs.
    -h, --help   Print this text.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("valley-artgen: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut out: Option<std::path::PathBuf> = None;
    let mut scale = 3u32;
    let mut check = false;

    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                let value = args.next().ok_or("`--out` needs a directory")?;
                out = Some(value.into());
            }
            "--scale" => {
                let value = args.next().ok_or("`--scale` needs a number")?;
                scale = value.parse().map_err(|_| format!("bad --scale: {value}"))?;
                if scale == 0 {
                    return Err("`--scale` must be at least 1".to_string());
                }
            }
            "--check" => check = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument {other:?}\n\n{USAGE}")),
        }
    }

    let out = out.ok_or("`--out DIR` is required")?;
    // `farm::write` writes into `<out>/farm`, so `root` is only for the messages.
    let root = out.join("farm");

    if check {
        // Regenerate into a scratch directory and compare, so the check never
        // writes into the tree it is inspecting.
        let scratch =
            std::env::temp_dir().join(format!("valley-artgen-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        farm::write(&scratch, scale).map_err(|e| e.to_string())?;
        let mut differing = Vec::new();
        let mut same = 0usize;
        for entry in std::fs::read_dir(scratch.join("farm")).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let expected = std::fs::read(entry.path()).map_err(|e| e.to_string())?;
            match std::fs::read(root.join(&name)) {
                Ok(on_disk) if on_disk == expected => same += 1,
                Ok(_) => differing.push(name.to_string_lossy().into_owned()),
                Err(_) => differing.push(format!("{} (missing)", name.to_string_lossy())),
            }
        }
        let _ = std::fs::remove_dir_all(&scratch);
        if differing.is_empty() {
            println!("valley-artgen: {same} files up to date");
            return Ok(());
        }
        differing.sort();
        return Err(format!(
            "{} files differ from a fresh generation:\n  {}",
            differing.len(),
            differing.join("\n  ")
        ));
    }

    let summary = farm::write(&out, scale).map_err(|e| e.to_string())?;
    println!("{}", summary.line(&out));
    println!(
        "valley-artgen: contact sheet -> {}",
        root.join("preview.png").display()
    );
    Ok(())
}
