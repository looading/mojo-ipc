fn main() -> Result<(), Box<dyn std::error::Error>> {
    mojom_build::Config::new()
        .source_root("schema")
        .source("control.mojom")
        .generate()?;
    Ok(())
}
