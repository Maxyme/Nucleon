use anyhow::Result;
use nucleon_core::paths;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

pub fn run(lines: usize, show_hook: bool, show_runner: bool, follow: bool) -> Result<()> {
    let hook_path = paths::support_dir().join("nucleon-hook.log");
    let runner_path = paths::support_dir().join("nucleon-runner.log");

    let targets: Vec<(&str, &PathBuf)> = if show_hook && !show_runner {
        vec![("Hook Log", &hook_path)]
    } else if show_runner && !show_hook {
        vec![("Runner Log", &runner_path)]
    } else {
        vec![("Hook Log", &hook_path), ("Runner Log", &runner_path)]
    };

    for (title, path) in &targets {
        println!("==> {} ({})", title, path.display());
        if path.is_file() {
            if let Ok(content) = fs::read_to_string(path) {
                let all_lines: Vec<&str> = content.lines().collect();
                let start = if all_lines.len() > lines {
                    all_lines.len() - lines
                } else {
                    0
                };
                for line in &all_lines[start..] {
                    println!("{line}");
                }
            }
        } else {
            println!("  (no log file exists yet)");
        }
        println!();
    }

    if follow {
        println!("==> Following logs (press Ctrl+C to exit)...");
        let active_path = if show_runner {
            &runner_path
        } else {
            &hook_path
        };
        if !active_path.exists() {
            let _ = File::create(active_path);
        }
        let mut file = File::open(active_path)?;
        let mut pos = file.seek(SeekFrom::End(0))?;

        loop {
            thread::sleep(Duration::from_millis(500));
            let metadata = fs::metadata(active_path)?;
            let len = metadata.len();
            if len > pos {
                file.seek(SeekFrom::Start(pos))?;
                let mut reader = BufReader::new(&file);
                let mut line = String::new();
                while reader.read_line(&mut line)? > 0 {
                    print!("{line}");
                    line.clear();
                }
                pos = file.stream_position()?;
            }
        }
    }

    Ok(())
}
