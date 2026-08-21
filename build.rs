//! Build script compiling the vendored tree-sitter-dockerfile grammar.
//!
//! The upstream crate release pins an incompatible tree-sitter core, so
//! we vendor its generated C sources (see vendor/tree-sitter-dockerfile)
//! and link them directly. The grammar targets language ABI 14, which
//! the tree-sitter 0.26 runtime loads without further ado.

fn main() {
    let src = "vendor/tree-sitter-dockerfile/src";
    cc::Build::new()
        .include(src)
        .file(format!("{src}/parser.c"))
        .file(format!("{src}/scanner.c"))
        .warnings(false)
        .compile("tree-sitter-dockerfile");
    println!("cargo:rerun-if-changed={src}");
    println!("cargo:rustc-env=KARTEI_BUILD_DATE={}", build_date());
    println!("cargo:rustc-env=KARTEI_BUILD_NUMBER={}", build_number());
    // Re-stamp date and number whenever a commit lands
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}

/// The build number: the git commit height of the repository.
///
/// Increments automatically with every commit; builds outside a git
/// checkout fall back to 0.
///
/// @return the commit count as build number
fn build_number() -> String {
    std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        })
        .unwrap_or_else(|| "0".to_string())
}

/// Format the current UTC time as ISO 8601 with seconds precision
/// (eg. `2026-08-04T12:34:56+00:00`) for the version flag.
///
/// @return the formatted build timestamp
fn build_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before 1970")
        .as_secs() as i64;
    let (hour, minute, second) =
        (secs % 86_400 / 3_600, secs % 3_600 / 60, secs % 60);

    // Convert the day count to a civil date without pulling in a date
    // crate for this one build-time stamp
    // See: https://howardhinnant.github.io/date_algorithms.html
    let z = secs / 86_400 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02}T\
         {hour:02}:{minute:02}:{second:02}+00:00"
    )
}
