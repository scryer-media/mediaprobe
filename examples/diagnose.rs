//! Prints the structural diagnosis of one file as JSON.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: diagnose <media-path>")?;
    let result =
        mediaprobe::diagnostics::diagnose_file(std::path::Path::new(&path), Default::default())?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
