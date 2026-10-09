use anyhow::Result;
use std::path::Path;

pub fn run(path: &Path) -> Result<()> {
    let info = nucleon_core::detector::detect_target_engine(path);
    let wine_eng = info.wine_engine();
    let lib_name = info.detected_dll.as_deref().unwrap_or("None (heuristic)");
    let gptk_name = nucleon_core::detector::TargetEngine::Gptk.display_name();

    println!(
        "==> Analyzing binary / game directory: {}\n  Graphics API detected:     {:?}\n  Detected library:          {}\n  Wine Auto Runner Pipeline: {}\n  GPTK Option Pipeline:      {}",
        path.display(),
        info.api,
        lib_name,
        wine_eng.display_name(),
        gptk_name
    );
    Ok(())
}
