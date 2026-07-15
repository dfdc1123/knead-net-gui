//! 从解析后的电路和板型生成布局算法所需的公共准备状态。

use crate::circuit::{Circuit, ComponentId, NetId};

use super::{Breadboard, PowerRailBinding, PowerRailBindings};

#[derive(Debug, Clone, Copy)]
pub enum PowerRailMatch {
    NotPresent,
    Unmatched,
    PositiveOnly(NetId),
    NegativeOnly(NetId),
    Bound(PowerRailBinding),
    IndividuallyBound(PowerRailBindings),
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
    prepare_for_layout_with_individual_power_nets(
        circuit,
        board,
        positive_name,
        negative_name,
        positive_name,
        negative_name,
    )
}

/// 使用用户为上下四条物理电源轨分别选择的网络名称准备布局。
///
/// 当上下同极性选择相同时保留预设短接线；选择不同时移除该短接线，避免把两个
/// 网络物理短路。四项都沿用自动匹配值时，行为与旧的两轨设置完全一致。
pub fn prepare_for_layout_with_individual_power_nets(
    circuit: &mut Circuit,
    board: Breadboard,
    top_positive_name: Option<&str>,
    top_negative_name: Option<&str>,
    bottom_positive_name: Option<&str>,
    bottom_negative_name: Option<&str>,
) -> LayoutPreparation {
    let find_net = |name: Option<&str>| {
        name.and_then(|name| {
            circuit
                .nets()
                .iter()
                .find(|net| net.name() == name)
                .map(|net| net.id())
        })
    };
    let bindings = PowerRailBindings {
        top: PowerRailBinding {
            positive: find_net(top_positive_name),
            negative: find_net(top_negative_name),
        },
        bottom: PowerRailBinding {
            positive: find_net(bottom_positive_name),
            negative: find_net(bottom_negative_name),
        },
    };

    prepare_for_layout_with_bindings(circuit, board, bindings)
}

fn prepare_for_layout_with_bindings(
    circuit: &mut Circuit,
    mut board: Breadboard,
    bindings: PowerRailBindings,
) -> LayoutPreparation {
    let power_names: Vec<String> = bindings
        .iter()
        .map(|(_, _, id)| id)
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

    let power_rails = if board.power_rails().is_none() {
        PowerRailMatch::NotPresent
    } else if bindings.is_empty() {
        PowerRailMatch::Unmatched
    } else if bindings.top == bindings.bottom {
        match (bindings.top.positive, bindings.top.negative) {
            (Some(_), Some(_)) => PowerRailMatch::Bound(bindings.top),
            (Some(positive), None) => PowerRailMatch::PositiveOnly(positive),
            (None, Some(negative)) => PowerRailMatch::NegativeOnly(negative),
            (None, None) => PowerRailMatch::Unmatched,
        }
    } else {
        PowerRailMatch::IndividuallyBound(bindings)
    };
    if !bindings.is_empty() && board.power_rails().is_some() {
        board = board.with_power_rail_bindings(bindings);
    }

    LayoutPreparation {
        board,
        bridgeable_components,
        power_rails,
    }
}

fn prepare_for_layout_with_net_ids(
    circuit: &mut Circuit,
    board: Breadboard,
    positive: Option<NetId>,
    negative: Option<NetId>,
) -> LayoutPreparation {
    prepare_for_layout_with_bindings(
        circuit,
        board,
        PowerRailBindings::mirrored(PowerRailBinding { positive, negative }),
    )
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
    fn four_physical_power_rails_can_use_independent_networks() {
        let mut circuit = power_and_signal_circuit();
        let prepared = prepare_for_layout_with_individual_power_nets(
            &mut circuit,
            super::super::Preset::Hole400.make(30),
            Some("SIGNAL"),
            Some("GND"),
            Some("VCC"),
            Some("SIGNAL"),
        );

        let PowerRailMatch::IndividuallyBound(bindings) = prepared.power_rails else {
            panic!("不同的上下绑定应保留逐物理轨语义");
        };
        assert_eq!(bindings.top.positive, Some(NetId(2)));
        assert_eq!(bindings.top.negative, Some(NetId(1)));
        assert_eq!(bindings.bottom.positive, Some(NetId(0)));
        assert_eq!(bindings.bottom.negative, Some(NetId(2)));
        assert!(prepared.board.rail_ties().is_empty());

        let actual: std::collections::HashMap<_, _> = prepared
            .board
            .bound_power_rail_anchors()
            .into_iter()
            .map(|(anchor, net)| (prepared.board.hole(anchor).position.y, net))
            .collect();
        assert_eq!(actual[&-4], NetId(1));
        assert_eq!(actual[&-3], NetId(2));
        assert_eq!(actual[&14], NetId(2));
        assert_eq!(actual[&15], NetId(0));
    }

    #[test]
    fn mirrored_defaults_keep_both_preset_ties() {
        let mut circuit = power_and_signal_circuit();
        let prepared = prepare_for_layout_with_individual_power_nets(
            &mut circuit,
            super::super::Preset::Hole400.make(30),
            Some("VCC"),
            Some("GND"),
            Some("VCC"),
            Some("GND"),
        );

        assert!(matches!(prepared.power_rails, PowerRailMatch::Bound(_)));
        assert_eq!(prepared.board.rail_ties().len(), 2);
        assert_eq!(
            prepared.board.power_rail_binding(),
            Some(&PowerRailBinding {
                positive: Some(NetId(0)),
                negative: Some(NetId(1)),
            })
        );
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
