use std::io::Result;

fn main() -> Result<()> {
    tonic_build::configure()
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .compile(&["src/echo.proto"], &["src"])?;
    Ok(())
}
