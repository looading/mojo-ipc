fn main() -> Result<(), Box<dyn std::error::Error>> {
    mojom_build::Config::new().source("echo.mojom").generate()?;
    Ok(())
}
