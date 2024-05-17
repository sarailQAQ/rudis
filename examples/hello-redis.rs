use std::time::Duration;
use rudis::{client, Result};

#[tokio::main]
async fn main() -> Result<()> {

    let mut client = client::connect("127.0.0.1:6379").await?;

    client.set("hello", "world".into(), Some(Duration::from_secs(60))).await?;
    let res = client.get("hello").await?;

    println!("result is {:?}", res);

    Ok(())
}
