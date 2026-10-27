//! Pairs with a cameo-pi and prints its state: a development check of the protocol.
//!   cargo run -p cutcraft-station --example pair -- ADDRESS
//! Approve the request on the Pi's web interface (Settings ▸ Programs) when the code shows.

use cutcraft_station::{Collected, Pairing, Station};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address = std::env::args().nth(1).ok_or("usage: pair ADDRESS")?;
    let pairing = Pairing::begin(&address, "CutCraft pairing check")?;
    println!(
        "Code {} (certificate {})",
        pairing.code, pairing.fingerprint
    );
    let credentials = loop {
        match pairing.collect()? {
            Collected::Waiting => std::thread::sleep(std::time::Duration::from_secs(1)),
            Collected::Paired(credentials) => break credentials,
            other => return Err(format!("not paired: {other:?}").into()),
        }
    };
    let station = Station::connect(credentials)?;
    let state = station.state()?;
    println!("cameo-pi {} (boot {})", state.version, state.boot);
    for cutter in state.cutters {
        println!(
            "  {} ({}): {}, mat {:?}",
            cutter.view.name, cutter.id, cutter.view.activity, cutter.view.mat
        );
    }
    Ok(())
}
