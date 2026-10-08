use std::fmt::Display;

/// Prints a section header: `==> {title}`
pub fn header(title: impl Display) {
    println!("==> {title}");
}

/// Prints a primary key-value status pair: `  {label:<26} {val}`
pub fn kv(label: impl Display, val: impl Display) {
    println!("  {:<26} {}", label, val);
}

/// Prints a sub-level key-value status pair: `    ● {label:<24} {val}`
pub fn sub_kv(label: impl Display, val: impl Display) {
    println!("    ● {:<24} {}", label, val);
}

/// Prints a nested tree item: `      {prefix} {label:<22} {val}`
pub fn tree_kv(prefix: &str, label: impl Display, val: impl Display) {
    println!("      {} {:<22} {}", prefix, label, val);
}

/// Prints a boolean status: `  {label:<26} ✓ Yes` or `✗ No`
pub fn status_bool(label: impl Display, ok: bool) {
    let val = if ok { "✓ Yes" } else { "✗ No" };
    println!("  {:<26} {}", label, val);
}

/// Prints a success step message: `  ✓ {msg}`
pub fn success(msg: impl Display) {
    println!("  ✓ {msg}");
}

/// Prints a warning or notice message: `  ! {msg}`
pub fn warn(msg: impl Display) {
    println!("  ! {msg}");
}

/// Prints an informational message: `  ○ {msg}`
pub fn info(msg: impl Display) {
    println!("  ○ {msg}");
}
