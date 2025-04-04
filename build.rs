use std::io::Result;
extern crate prost_build;

fn main() -> Result<()> {
    let protos = &[
        "src/proto/api.proto",
    ];
    prost_build::compile_protos(protos, &["src/proto/"])?;
    Ok(())
}