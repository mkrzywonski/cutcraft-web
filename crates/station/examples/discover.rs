//! Lists the cameo-pis that answer on this network: a development check of discovery.
//!   cargo run -p cutcraft-station --example discover

fn main() -> Result<(), String> {
    for pi in cutcraft_station::discover(std::time::Duration::from_secs(3))? {
        println!("{}: {} ({})", pi.name, pi.address, pi.ips.join(", "));
    }
    Ok(())
}
