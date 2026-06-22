use serde::Deserialize;
use std::fs;

// 这一层处理JSON input
#[derive(Debug, Deserialize)]
struct CircuitInput {
    components: Vec<ComponentInput>,
    nets: Vec<NetInput>,
}

#[derive(Debug, Deserialize)]
struct ComponentInput {
    id: String,
    kind: String,
    pins: Vec<PinInput>,
}

#[derive(Debug, Deserialize)]
struct PinInput {
    name: String,
}

#[derive(Debug, Deserialize)]
struct NetInput {
    id: String,
    connections: Vec<ConnectionInput>,
}

#[derive(Debug, Deserialize)]
struct ConnectionInput {
    component_name: String,
    pin_name: String,
}

// 让pin有所属的component
struct ComponentId(usize);

struct PinId(usize);

struct NetId(usize);

struct Circuit {
    components: Vec<Component>,
    pins: Vec<Pin>,
    nets: Vec<Net>,
}

struct Component {
    id: ComponentId,
    name: String,
    kind: String,
    pins: Vec<PinId>,
}

struct Pin {
    id: PinId,

    component: ComponentId,

    name: String,

    net: Option<NetId>,
}

struct Net {
    id: NetId,

    name: String,

    pins: Vec<PinId>,
}

fn main() {
    let json = fs::read_to_string("examples/led_bjt.json").unwrap();

    let circuit: CircuitInput = serde_json::from_str(&json).unwrap();

    println!("{:#?}", circuit);
}
