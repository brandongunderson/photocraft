//! Embed only the two public SDK source packages. Installed tools need no source checkout.
#![forbid(unsafe_code)]
use std::{
    error::Error,
    path::{Path, PathBuf},
};

fn collect(directory: &Path, depth: usize, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    if depth > 16 || files.len() > 1024 {
        return Err("SDK source tree exceeds build limits".into());
    }
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err("SDK source must not contain links".into());
        }
        if kind.is_dir() {
            collect(&entry.path(), depth + 1, files)?;
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest directory")?).join("../../crates");
    let mut output = String::from("pub const SDK_FILES: &[(&str, &[u8])] = &[\n");
    for name in ["plugin-api", "plugin-sdk"] {
        let package = root.join(name).canonicalize()?;
        let mut files = ["Cargo.toml", "README.md", "LICENSE-MIT", "LICENSE-APACHE"].map(|file| package.join(file)).to_vec();
        for directory in ["src", "tests", "examples", "schemas"] {
            if package.join(directory).is_dir() {
                collect(&package.join(directory), 0, &mut files)?;
            }
        }
        files.sort();
        for file in files {
            let relative = file.strip_prefix(&package)?.to_str().ok_or("non-UTF8 SDK path")?.replace('\\', "/");
            let source = file.to_str().ok_or("non-UTF8 SDK source")?;
            println!("cargo:rerun-if-changed={source}");
            output.push_str(&format!("({:?}, include_bytes!({source:?})),\n", format!("{name}/{relative}")));
        }
        println!("cargo:rerun-if-changed={}", package.display());
    }
    output.push_str("];\n");
    let destination = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("missing build output directory")?).join("sdk_sources.rs");
    std::fs::write(destination, output)?;
    Ok(())
}
