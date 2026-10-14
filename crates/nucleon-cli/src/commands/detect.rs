use anyhow::Result;
use std::path::Path;

pub fn run(path: &Path) -> Result<()> {
    let info = nucleon_core::detector::detect_target_engine(path);
    let wine_eng = info.auto_engine()?;
    let lib_name = info.detected_dll.as_deref().unwrap_or("None");
    let gptk_name = nucleon_core::detector::TargetEngine::Gptk.display_name();

    println!(
        "==> Analyzing binary / game directory: {}\n  Graphics binary:           {}\n  Graphics API detected:     {:?}\n  Detected library:          {}\n  Detection evidence:        {}\n  Automatic Runner Pipeline: {}\n  GPTK Option Pipeline:      {}",
        path.display(),
        info.binary.as_deref().unwrap_or(path).display(),
        info.api,
        lib_name,
        info.evidence.description(),
        wine_eng.display_name(),
        gptk_name
    );
    Ok(())
}
