use std::fs;

use knead_net::Circuit;
use knead_net::input::json::CircuitInput;

fn main() {
    let json = fs::read_to_string("examples/led_bjt.json").unwrap();

    let input: CircuitInput = serde_json::from_str(&json).unwrap();
    println!("{input:#?}");

    let circuit = Circuit::from(input);
    println!("{circuit:#?}");
}
