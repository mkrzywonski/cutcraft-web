//! Writes the authored test job (a 40 mm square) as `square.cutcraft` and `square-plan.json` in
//! the folder given, for trying the server by hand:
//! `cargo run -p cameo-pi-core --features fixtures --example write_fixture -- DIR`.

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let (project, plan) = cameo_pi_core::fixtures::upload();
    std::fs::write(format!("{dir}/square.cutcraft"), project).expect("write the project");
    std::fs::write(format!("{dir}/square-plan.json"), plan).expect("write the plan");
    println!("{}", cameo_pi_core::validate::PLANNER);
}
