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
/// 该函数保持原有自动匹配策略，并把匹配到的每条电源轨独立绑定。
pub fn prepare_for_layout(circuit: &mut Circuit, board: Breadboard) -> LayoutPreparation {
    let positive = board
        .positive_names()
        .iter()
        .find_map(|name| circuit.nets().iter().find(|net| net.name() == name));
    let negative = board
        .negative_names()
        .iter()
        .find_map(|name| circuit.nets().iter().find(|net| net.name() == name));

    prepare_for_layout_with_net_ids(
        circuit,
        board,
        positive.map(|net| net.id()),
        negative.map(|net| net.id()),
    )
}

/// 使用用户选择的网络名称准备布局。每一条电源轨都可以独立绑定或留空。
///
/// 名称按当前 PCB 的网络精确匹配；不存在的名称按未绑定处理。调用方若需要把
/// 过期名称视为错误，应在调用前校验。
pub fn prepare_for_layout_with_power_nets(
    circuit: &mut Circuit,
    board: Breadboard,
    positive_name: Option<&str>,
    negative_name: Option<&str>,
) -> LayoutPreparation {
    let positive = positive_name.and_then(|name| {
        circuit
            .nets()
            .iter()
            .find(|net| net.name() == name)
            .map(|net| net.id())
    });
    let negative = negative_name.and_then(|name| {
        circuit
            .nets()
            .iter()
            .find(|net| net.name() == name)
            .map(|net| net.id())
    });

    prepare_for_layout_with_net_ids(circuit, board, positive, negative)
}

fn prepare_for_layout_with_net_ids(
    circuit: &mut Circuit,
    board: Breadboard,
    positive: Option<NetId>,
    negative: Option<NetId>,
) -> LayoutPreparation {
    let power_names: Vec<String> = [negative, positive]
        .into_iter()
        .flatten()
        .map(|id| circuit.nets()[id.0].name().to_string())
        .collect();
    let power_name_refs: Vec<&str> = power_names.iter().map(String::as_str).collect();
    crate::input::pcb::auto_mark_bridgeable(circuit, &power_name_refs);

    let bridgeable_components = circuit
        .components()
        .iter()
        .filter(|component| component.bridgeable)
        .map(|component| component.id())
        .collect();

    let (board, power_rails) = match (positive, negative) {
        (Some(positive), Some(negative)) => {
            let binding = PowerRailBinding {
                positive: Some(positive),
                negative: Some(negative),
            };
            (
                board.with_power_rail_binding(binding),
                PowerRailMatch::Bound(binding),
            )
        }
        (Some(positive), None) => (
            board.with_power_rail_binding(PowerRailBinding {
                positive: Some(positive),
                negative: None,
            }),
            PowerRailMatch::PositiveOnly(positive),
        ),
        (None, Some(negative)) => (
            board.with_power_rail_binding(PowerRailBinding {
                positive: None,
                negative: Some(negative),
            }),
            PowerRailMatch::NegativeOnly(negative),
        ),
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
        assert_eq!(binding.positive, Some(NetId(0)));
        assert_eq!(binding.negative, Some(NetId(1)));
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

    #[test]
    fn explicit_single_rail_binding_is_applied() {
        let mut circuit = power_and_signal_circuit();
        let prepared = prepare_for_layout_with_power_nets(
            &mut circuit,
            super::super::Preset::Hole400.make(30),
            Some("SIGNAL"),
            None,
        );

        assert!(matches!(
            prepared.power_rails,
            PowerRailMatch::PositiveOnly(NetId(2))
        ));
        let binding = prepared.board.power_rail_binding().unwrap();
        assert_eq!(binding.positive, Some(NetId(2)));
        assert_eq!(binding.negative, None);
        assert_eq!(prepared.bridgeable_components, vec![ComponentId(0)]);
    }

    #[test]
    fn explicit_names_can_reverse_the_automatic_polarity_choice() {
        let mut circuit = power_and_signal_circuit();
        let prepared = prepare_for_layout_with_power_nets(
            &mut circuit,
            super::super::Preset::Hole400.make(30),
            Some("SIGNAL"),
            Some("VCC"),
        );

        let PowerRailMatch::Bound(binding) = prepared.power_rails else {
            panic!("both selected rails should be bound");
        };
        assert_eq!(binding.positive, Some(NetId(2)));
        assert_eq!(binding.negative, Some(NetId(0)));
        assert!(prepared.bridgeable_components.is_empty());
    }

    #[test]
    fn repeated_prepare_recomputes_bridgeability_from_current_binding() {
        let mut circuit = power_and_signal_circuit();

        let first = prepare_for_layout_with_power_nets(
            &mut circuit,
            super::super::Preset::Hole400.make(30),
            Some("VCC"),
            None,
        );
        assert_eq!(first.bridgeable_components, vec![ComponentId(0)]);
        assert!(circuit.components()[0].bridgeable);

        let changed = prepare_for_layout_with_power_nets(
            &mut circuit,
            super::super::Preset::Hole400.make(30),
            Some("VCC"),
            Some("SIGNAL"),
        );
        assert!(
            changed.bridgeable_components.is_empty(),
            "两只脚都属于当前 power nets 时不能残留上一次的 eligibility"
        );
        assert!(!circuit.components()[0].bridgeable);

        let repeated = prepare_for_layout_with_power_nets(
            &mut circuit,
            super::super::Preset::Hole400.make(30),
            Some("VCC"),
            None,
        );
        assert_eq!(repeated.bridgeable_components, first.bridgeable_components);
        assert!(circuit.components()[0].bridgeable);
    }
}
