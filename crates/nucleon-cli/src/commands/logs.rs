use anyhow::Result;
use nucleon_core::paths;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

fn current_runner_path() -> Result<PathBuf> {
    Ok(paths::latest_run_dir()?
        .map(|p| p.join("runner.log"))
        .unwrap_or_else(|| paths::support_dir().join("nucleon-runner.log")))
}

pub fn run(lines: usize, follow: bool) -> Result<()> {
    let runner_path = current_runner_path()?;
    let targets = [("Runner Log", &runner_path)];

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
        let mut active_path = runner_path;
        let mut file = if active_path.is_file() {
            Some(File::open(&active_path)?)
        } else {
            None
        };
        let mut pos = match &mut file {
            Some(file) => file.seek(SeekFrom::End(0))?,
            None => 0,
        };

        loop {
            thread::sleep(Duration::from_millis(500));
            {
                let latest = current_runner_path()?;
                if latest != active_path {
                    println!("==> Runner Log ({})", latest.display());
                    active_path = latest;
                    file = None;
                    pos = 0;
                }
            }
            if file.is_none() {
                match File::open(&active_path) {
                    Ok(opened) => file = Some(opened),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            let file = file.as_mut().unwrap();
            let metadata = file.metadata()?;
            let len = metadata.len();
            if len < pos {
                pos = 0;
            }
            if len > pos {
                file.seek(SeekFrom::Start(pos))?;
                let mut reader = BufReader::new(&mut *file);
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
