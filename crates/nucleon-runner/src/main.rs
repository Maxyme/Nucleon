use anyhow::Result;
use std::env;

fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = env::args().collect();
    nucleon_runner::run_with_args(&args)
}
