//! Prints the probe result of one file as JSON.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: probe <media-path>")?;
    let analysis = mediaprobe::probe(std::path::Path::new(&path))?;
    println!("{}", serde_json::to_string_pretty(&analysis)?);
    Ok(())
}
