//! Run with `--features schema --example export_schemas -- <output-directory> ...`.

use std::{error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let outputs = std::env::args_os().skip(1).map(PathBuf::from).collect::<Vec<_>>();
    if outputs.is_empty() {
        return Err("supply at least one output directory".into());
    }
    for output in outputs {
        std::fs::create_dir_all(&output)?;
        for (name, schema) in photocraft_plugin_api::schema::all() {
            let bytes = serde_json::to_vec_pretty(&schema)?;
            std::fs::write(output.join(name), bytes)?;
        }
    }
    Ok(())
}
