//! 从解析后的电路和板型生成布局算法所需的公共准备状态。

use crate::circuit::{Circuit, ComponentId, NetId};

use super::{Breadboard, PowerRailBinding};

#[derive(Debug, Clone, Copy)]
pub enum PowerRailMatch {
    NotPresent,
    Unmatched,
    PositiveOnly(NetId),
    NegativeOnly(NetId),
    Bound(PowerRailBinding),
}

#[derive(Debug)]
pub struct LayoutPreparation {
    pub board: Breadboard,
    pub bridgeable_components: Vec<ComponentId>,
    pub power_rails: PowerRailMatch,
}

/// 执行布局前必须完成的通用准备。
///
/// 该函数保持原有策略：先按板子的正/负电源名称标记可桥接的两脚元件；只有
/// 正负两条网络都匹配成功时才真正绑定电源轨，单边匹配只写入报告。
pub fn prepare_for_layout(circuit: &mut Circuit, board: Breadboard) -> LayoutPreparation {
    let power_names: Vec<String> = board
        .positive_names()
        .iter()
        .chain(board.negative_names().iter())
        .cloned()
        .collect();
    let power_name_refs: Vec<&str> = power_names.iter().map(String::as_str).collect();
    crate::input::pcb::auto_mark_bridgeable(circuit, &power_name_refs);

    let bridgeable_components = circuit
        .components()
        .iter()
        .filter(|component| component.bridgeable)
        .map(|component| component.id())
        .collect();

    let positive = board
        .positive_names()
        .iter()
        .find_map(|name| circuit.nets().iter().find(|net| net.name() == name));
    let negative = board
        .negative_names()
        .iter()
        .find_map(|name| circuit.nets().iter().find(|net| net.name() == name));

    let (board, power_rails) = match (positive, negative) {
        (Some(positive), Some(negative)) => {
            let binding = PowerRailBinding {
                positive: positive.id(),
                negative: negative.id(),
            };
            (
                board.with_power_rail_binding(binding),
                PowerRailMatch::Bound(binding),
            )
        }
        (Some(positive), None) => (board, PowerRailMatch::PositiveOnly(positive.id())),
        (None, Some(negative)) => (board, PowerRailMatch::NegativeOnly(negative.id())),
        (None, None) if board.power_rails().is_some() => (board, PowerRailMatch::Unmatched),
        (None, None) => (board, PowerRailMatch::NotPresent),
    };

    LayoutPreparation {
        board,
        bridgeable_components,
        power_rails,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{Component, Net, Pin, PinId};

    fn power_and_signal_circuit() -> Circuit {
        Circuit {
            components: vec![Component {
                id: ComponentId(0),
                ref_: "R1".into(),
                kind: "R".into(),
                value: Some("10k".into()),
                pins: vec![PinId(0), PinId(1)],
                footprint: None,
                bridgeable: false,
            }],
            pins: vec![
                Pin {
                    id: PinId(0),
                    component: ComponentId(0),
                    num: "1".into(),
                    pinfunction: None,
                    net: Some(NetId(0)),
                    physical_pin_index: 0,
                },
                Pin {
                    id: PinId(1),
                    component: ComponentId(0),
                    num: "2".into(),
                    pinfunction: None,
                    net: Some(NetId(2)),
                    physical_pin_index: 1,
                },
            ],
            nets: vec![
                Net {
                    id: NetId(0),
                    name: "VCC".into(),
                    pins: vec![PinId(0)],
                },
                Net {
                    id: NetId(1),
                    name: "GND".into(),
                    pins: vec![],
                },
                Net {
                    id: NetId(2),
                    name: "SIGNAL".into(),
                    pins: vec![PinId(1)],
                },
            ],
            footprints: vec![],
        }
    }

    #[test]
    fn prepares_bridgeable_components_and_binds_both_power_rails() {
        let mut circuit = power_and_signal_circuit();
        let prepared = prepare_for_layout(&mut circuit, super::super::Preset::Hole400.make(30));

        assert_eq!(prepared.bridgeable_components, vec![ComponentId(0)]);
        assert!(circuit.components()[0].bridgeable);
        let PowerRailMatch::Bound(binding) = prepared.power_rails else {
            panic!("VCC/GND 都存在时应绑定电源轨");
        };
        assert_eq!(binding.positive, NetId(0));
        assert_eq!(binding.negative, NetId(1));
        assert!(prepared.board.power_rail_binding().is_some());
    }

    #[test]
    fn board_without_power_rails_is_reported_without_binding() {
        let mut circuit = power_and_signal_circuit();
        let prepared = prepare_for_layout(&mut circuit, super::super::Preset::Hole170.make(17));

        assert!(matches!(prepared.power_rails, PowerRailMatch::NotPresent));
        assert!(prepared.board.power_rail_binding().is_none());
        assert!(prepared.bridgeable_components.is_empty());
    }
}
