//! JSON 输入格式及其到 [`Circuit`] 的转换。

use std::collections::HashMap;

use serde::Deserialize;

use crate::circuit::{Circuit, Component, ComponentId, Net, NetId, Pin, PinId};

// 这一层处理 JSON input
#[derive(Debug, Deserialize)]
pub struct CircuitInput {
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
    component_id: String,
    pin_name: String,
}

impl From<CircuitInput> for Circuit {
    fn from(input: CircuitInput) -> Self {
        // 建立 component
        let mut components: Vec<Component> = Vec::with_capacity(input.components.len());
        let mut pins: Vec<Pin> = Vec::new();
        // 用 (component_name, pin_name) -> PinId 索引, 解析 net 的连接时用
        let mut pin_lookup: HashMap<(String, String), PinId> = HashMap::new();

        for comp_in in input.components {
            let component_id = ComponentId(components.len());
            let comp_name = comp_in.id.clone();
            let mut comp_pin_ids: Vec<PinId> = Vec::with_capacity(comp_in.pins.len());

            // 建立 pin
            for pin_in in comp_in.pins {
                let pin_id = PinId(pins.len());
                pins.push(Pin {
                    id: pin_id,
                    component: component_id,
                    num: pin_in.name.clone(),
                    pinfunction: None,
                    net: None,
                });
                pin_lookup.insert((comp_name.clone(), pin_in.name), pin_id);
                comp_pin_ids.push(pin_id);
            }

            components.push(Component {
                id: component_id,
                ref_: comp_name,
                kind: comp_in.kind,
                value: None,
                pins: comp_pin_ids,
                footprint: None,
                bridgeable: false,
            });
        }

        // 建立 net
        let mut nets: Vec<Net> = Vec::with_capacity(input.nets.len());
        for net_in in input.nets {
            let net_id = NetId(nets.len());
            let mut net_pins: Vec<PinId> = Vec::with_capacity(net_in.connections.len());

            // 建立连接关系
            for conn in net_in.connections {
                let pin_id = pin_lookup
                    .get(&(conn.component_id.clone(), conn.pin_name.clone()))
                    .copied()
                    .unwrap_or_else(|| {
                        panic!(
                            "net '{}' 引用了未知的 pin '{}.{}'",
                            net_in.id, conn.component_id, conn.pin_name
                        )
                    });
                pins[pin_id.0].net = Some(net_id);
                net_pins.push(pin_id);
            }

            nets.push(Net {
                id: net_id,
                name: net_in.id,
                pins: net_pins,
            });
        }

        Circuit {
            components,
            pins,
            nets,
            footprints: Vec::new(),
        }
    }
}
