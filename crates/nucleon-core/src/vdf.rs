use std::fs;
use std::path::Path;
use anyhow::{Context, Result};

pub fn generate_compatibilitytool_vdf() -> String {
    r#""compatibilitytools"
{
  "compat_tools"
  {
    "nucleon"
    {
      "install_path" "."
      "entry_here" "nucleon-runner"
      "display_name" "Nucleon (Game Porting Toolkit 4)"
      "from_oslist" "windows"
      "to_oslist" "macos"
    }
  }
}
"#
    .to_string()
}

pub fn write_compatibilitytool_vdf(target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = generate_compatibilitytool_vdf();
    fs::write(target, content)
        .with_context(|| format!("Failed to write compatibilitytool.vdf at {}", target.display()))?;
    Ok(())
}
