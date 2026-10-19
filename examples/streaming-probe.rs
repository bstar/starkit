//! Standalone real-codec integration fixture; never opens audio or a compositor.
use starkit::streaming::{Resource, Server};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
};
fn main() -> anyhow::Result<()> {
    let source = PathBuf::from(std::env::args_os().nth(1).expect("movie path"));
    let root = source.parent().unwrap();
    let resource = Resource::new(PathBuf::from(source.file_name().unwrap()))?;
    let catalog = serde_json::json!({"id":resource.id});
    let server = Server::start(root, vec![resource], catalog, "d".repeat(32))?;
    println!("{}", serde_json::to_string(&server.session)?);
    io::stdout().flush()?;
    let _ = io::stdin().read(&mut [0])?;
    drop(server);
    Ok(())
}
