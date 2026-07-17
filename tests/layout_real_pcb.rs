use std::collections::HashMap;

use knead_net::input::pcb::parse_pcb;
use knead_net::{
    BridgeInitial, BridgePolicy, ComponentId, HoleId, Layout, NetId, PathFinderRouter, Placement,
    Position, Preset, Rotation, Router, SAConfig, prepare_for_layout,
    prepare_for_layout_with_individual_power_nets,
};

const SA_ALGORITHM_VERSION: &str = "initializer-families-v1";
const FIXED_SEED: u64 = 0x14_2026_0715;

#[derive(Debug, PartialEq, Eq)]
struct FixtureResult {
    placements: Vec<Option<Placement>>,
    wires: Vec<(usize, usize, usize)>,
}

#[derive(Debug)]
struct DisjointSet {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl DisjointSet {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
            rank: vec![0; len],
        }
    }

    fn find(&mut self, node: usize) -> usize {
        if self.parent[node] != node {
            self.parent[node] = self.find(self.parent[node]);
        }
        self.parent[node]
    }

    fn union(&mut self, a: usize, b: usize) {
        let mut a = self.find(a);
        let mut b = self.find(b);
        if a == b {
            return;
        }
        if self.rank[a] < self.rank[b] {
            std::mem::swap(&mut a, &mut b);
        }
        self.parent[b] = a;
        if self.rank[a] == self.rank[b] {
            self.rank[a] += 1;
        }
    }
}

fn run_real_fixture() -> FixtureResult {
    let mut circuit = parse_pcb(include_str!("../examples/inputs/h-bridge.kicad_pcb"))
        .expect("real h-bridge PCB fixture should parse");
    let prepared = prepare_for_layout(&mut circuit, Preset::Hole830.make(63));
    let board = prepared.board;
    assert!(
        board.power_rail_binding().is_some(),
        "fixture must exercise automatic power binding"
    );
    assert!(
        !prepared.bridgeable_components.is_empty(),
        "fixture must contain bridge candidates"
    );

    let mut layout = Layout::new(&circuit);
    layout
        .place_sa(
            &board,
            &SAConfig {
                max_iters: 5_000,
                seed: FIXED_SEED,
                use_spectral: true,
                bridge_policy: BridgePolicy::Explore {
                    initial: BridgeInitial::BestOfBoth,
                },
                ..SAConfig::default()
            },
        )
        .expect("real fixture placement should be legal");
    layout
        .route_with_progress(&board, &PathFinderRouter::default(), |_| {})
        .expect("real fixture route should validate");
    layout
        .validate(&board)
        .expect("routed fixture should validate");

    assert!(
        layout
            .placements()
            .iter()
            .flatten()
            .any(Placement::is_bridged),
        "fixture and fixed seed must cover a Bridged pose"
    );
    assert!(
        layout
            .placements()
            .iter()
            .flatten()
            .any(|placement| matches!(placement, Placement::OnBoard { .. })),
        "fixture and fixed seed must cover an OnBoard pose"
    );
    assert!(!layout.wires().is_empty(), "fixture must exercise routing");

    assert_effective_connectivity(&layout, &board);

    let placements = layout.placements().to_vec();
    let wires = layout
        .wires()
        .iter()
        .map(|wire| (wire.net.raw(), wire.from.raw(), wire.to.raw()))
        .collect();
    FixtureResult { placements, wires }
}

fn assert_effective_connectivity(layout: &Layout<'_>, board: &knead_net::Breadboard) {
    let circuit = layout.circuit();
    let mut graph = DisjointSet::new(board.len());
    let mut island_representatives = HashMap::<u32, HoleId>::new();
    for hole in board.holes() {
        let effective_island = board.effective_rail_id_of(hole.id);
        if let Some(representative) = island_representatives.insert(effective_island, hole.id) {
            graph.union(representative.raw(), hole.id.raw());
        }
    }
    for wire in layout.wires() {
        graph.union(wire.from.raw(), wire.to.raw());
    }

    let mut endpoints_by_net = vec![Vec::<HoleId>::new(); circuit.nets().len()];
    for component in circuit.components() {
        let placement = layout
            .placement(component.id())
            .expect("every parsed through-hole component must be placed");
        let footprint = &circuit.footprints()[component
            .footprint()
            .expect("parsed component must have a footprint")
            .raw()];
        let placed = placement
            .apply(component, footprint, board, circuit.pins())
            .expect("committed placement must apply");
        for pin_hole in placed.pin_holes {
            if let Some(net) = circuit.pins()[pin_hole.pin.raw()].net() {
                endpoints_by_net[net.raw()].push(pin_hole.hole);
            }
        }
    }

    if let Some(binding) = board.power_rail_binding() {
        for (polarity, net) in binding.iter() {
            let anchors = board
                .power_rail_anchors(polarity)
                .expect("bound polarity must have top and bottom anchors");
            endpoints_by_net[net.raw()].extend(anchors);
        }
    }
    for wire in layout.wires() {
        endpoints_by_net[wire.net.raw()].extend(wire.contacts());
    }

    let mut owner_by_component = HashMap::<usize, NetId>::new();
    for net in circuit.nets() {
        let endpoints = &endpoints_by_net[net.id().raw()];
        assert!(
            !endpoints.is_empty(),
            "net {:?} must retain at least one physical endpoint",
            net.name()
        );
        let expected_component = graph.find(endpoints[0].raw());
        for &endpoint in &endpoints[1..] {
            assert_eq!(
                graph.find(endpoint.raw()),
                expected_component,
                "net {:?} is split in the effective connectivity graph",
                net.name()
            );
        }
        if let Some(other) = owner_by_component.insert(expected_component, net.id()) {
            assert_eq!(
                other,
                net.id(),
                "nets {:?} and {:?} share one effective conductive component",
                circuit.nets()[other.raw()].name(),
                net.name()
            );
        }
    }
}

#[test]
fn real_pcb_parse_prepare_sa_route_connectivity() {
    let first = run_real_fixture();
    let second = run_real_fixture();
    assert_eq!(
        first.placements, second.placements,
        "algorithm {SA_ALGORITHM_VERSION} with a fixed seed must reproduce placements"
    );
    assert_eq!(
        first.wires, second.wires,
        "algorithm {SA_ALGORITHM_VERSION} with a fixed seed must reproduce routing"
    );
}

fn different_order_circuit() -> knead_net::Circuit {
    parse_pcb(include_str!(
        "../examples/folders/h-bridge_different_order/h-bridge.kicad_pcb"
    ))
    .expect("different-order h-bridge PCB fixture should parse")
}

fn component_id(circuit: &knead_net::Circuit, reference: &str) -> ComponentId {
    circuit
        .components()
        .iter()
        .find(|component| component.ref_() == reference)
        .unwrap_or_else(|| panic!("missing component {reference}"))
        .id()
}

fn pin_id(circuit: &knead_net::Circuit, component: ComponentId, number: &str) -> knead_net::PinId {
    circuit.components()[component.raw()]
        .pins()
        .iter()
        .copied()
        .find(|pin| circuit.pins()[pin.raw()].num() == number)
        .unwrap_or_else(|| panic!("missing pin {number}"))
}

fn place_reported_unroutable_solution(
    circuit: &knead_net::Circuit,
    board: &knead_net::Breadboard,
    layout: &mut Layout<'_>,
) {
    for (reference, x, y, rotation) in [
        ("D4", 12, 2, Rotation::R0),
        ("Q2", 22, 0, Rotation::R180),
        ("Q4", 5, 1, Rotation::R180),
        ("R7", 11, 4, Rotation::R180),
        ("Q3", 18, 1, Rotation::R180),
        ("R8", 6, 3, Rotation::R180),
        ("Q6", 14, 3, Rotation::R0),
        ("R5", 0, 2, Rotation::R0),
        ("Q5", 12, 1, Rotation::R180),
        ("Q1", 25, 3, Rotation::R180),
        ("R1", 28, 4, Rotation::R180),
        ("R4", 17, 4, Rotation::R180),
        ("R3", 21, 1, Rotation::R0),
    ] {
        layout.place(
            component_id(circuit, reference),
            Placement::OnBoard {
                position: Position { x, y },
                rotation,
            },
        );
    }
    for (reference, first_pin, first, second_pin, second) in [
        ("D2", "2", (18, -4), "1", (18, 0)),
        ("R2", "2", (26, -3), "1", (26, 1)),
        ("D3", "1", (14, -3), "2", (14, 1)),
        ("D1", "1", (19, -3), "2", (19, 1)),
        ("R6", "2", (7, -3), "1", (7, 1)),
    ] {
        let component = component_id(circuit, reference);
        layout.place(
            component,
            Placement::Bridged {
                pin_holes: [
                    (
                        board.at(first.0, first.1).expect("first bridge hole"),
                        pin_id(circuit, component, first_pin),
                    ),
                    (
                        board.at(second.0, second.1).expect("second bridge hole"),
                        pin_id(circuit, component, second_pin),
                    ),
                ],
            },
        );
    }
}

#[test]
fn different_order_reported_solution_is_rejected_as_disconnected() {
    use std::cell::Cell;

    let mut circuit = different_order_circuit();
    let prepared = prepare_for_layout_with_individual_power_nets(
        &mut circuit,
        Preset::Hole400.make_repeated_upper_half(2),
        Some("+12V"),
        Some("GND"),
        None,
        None,
    );
    let board = prepared.board;
    let mut layout = Layout::new(&circuit);
    place_reported_unroutable_solution(&circuit, &board, &mut layout);
    layout
        .validate(&board)
        .expect("reported placement is structurally legal");
    let port_errors = layout
        .validate_routing_ports(&board)
        .expect_err("full column 15 must be hard-unroutable");
    assert!(port_errors.iter().any(|error| {
        matches!(
            error,
            knead_net::LayoutError::InsufficientRoutingPorts {
                net,
                effective_rail: Some(_),
                available: 0,
                required: 1,
            } if circuit.nets()[net.raw()].name() == "Net-(D3-A)"
        )
    }));
    let occupancy = layout.occupancy(&board).expect("placement occupancy");
    let existing =
        PathFinderRouter::default().route(&circuit, &board, &occupancy, &layout.bridged_pins());
    layout.add_wire(existing[0].clone());
    let original_wires = layout
        .wires()
        .iter()
        .map(|wire| (wire.net.raw(), wire.from.raw(), wire.to.raw()))
        .collect::<Vec<_>>();

    let completed = Cell::new(false);
    let result = layout.route_with_progress(&board, &PathFinderRouter::default(), |_| {
        completed.set(true);
    });

    assert!(result.is_err(), "a split Net-(D3-A) must not be accepted");
    assert!(
        !completed.get(),
        "failed routing must not publish completion"
    );
    assert_eq!(
        layout
            .wires()
            .iter()
            .map(|wire| (wire.net.raw(), wire.from.raw(), wire.to.raw()))
            .collect::<Vec<_>>(),
        original_wires,
        "failed routing must preserve the previous wires"
    );
}

#[derive(Debug, PartialEq, Eq)]
struct NormalizedResult {
    placements: Vec<(String, String)>,
    wires: Vec<NormalizedWire>,
}

type NormalizedWire = (String, (i32, i32), (i32, i32));

fn normalized_order_result(source: &str) -> NormalizedResult {
    let mut circuit = parse_pcb(source).expect("fixture should parse");
    let prepared = prepare_for_layout_with_individual_power_nets(
        &mut circuit,
        Preset::Hole400.make_repeated_upper_half(2),
        Some("+12V"),
        Some("GND"),
        None,
        None,
    );
    let board = prepared.board;
    let mut layout = Layout::new(&circuit);
    layout
        .place_sa(
            &board,
            &SAConfig {
                max_iters: 2_000,
                n_seeds: 8,
                seed: 0xD1FF_E2E0,
                use_spectral: true,
                bridge_policy: BridgePolicy::Explore {
                    initial: BridgeInitial::BestOfBoth,
                },
                ..SAConfig::default()
            },
        )
        .expect("placement should succeed");
    layout
        .route_with_progress(&board, &PathFinderRouter::default(), |_| {})
        .expect("routing should succeed");

    let mut placements = Vec::new();
    for component in circuit.components() {
        let placement = layout.placement(component.id()).expect("placed component");
        let normalized = match placement {
            Placement::OnBoard { position, rotation } => {
                format!("onboard:{},{}:{rotation:?}", position.x, position.y)
            }
            Placement::Bridged { pin_holes } => {
                let mut pins = pin_holes
                    .iter()
                    .map(|(hole, pin)| {
                        let position = board.hole(*hole).position;
                        format!(
                            "{}@{},{}",
                            circuit.pins()[pin.raw()].num(),
                            position.x,
                            position.y
                        )
                    })
                    .collect::<Vec<_>>();
                pins.sort();
                format!("bridged:{}", pins.join(";"))
            }
        };
        placements.push((component.ref_().to_string(), normalized));
    }
    placements.sort();

    let mut wires = layout
        .wires()
        .iter()
        .map(|wire| {
            let mut endpoints = [board.hole(wire.from).position, board.hole(wire.to).position];
            endpoints.sort_by_key(|position| (position.x, position.y));
            (
                circuit.nets()[wire.net.raw()].name().to_string(),
                (endpoints[0].x, endpoints[0].y),
                (endpoints[1].x, endpoints[1].y),
            )
        })
        .collect::<Vec<_>>();
    wires.sort();
    NormalizedResult { placements, wires }
}

#[test]
fn reordered_kicad_footprints_keep_fixed_seed_result() {
    let original = normalized_order_result(include_str!("../examples/inputs/h-bridge.kicad_pcb"));
    let reordered = normalized_order_result(include_str!(
        "../examples/folders/h-bridge_different_order/h-bridge.kicad_pcb"
    ));
    assert_eq!(original, reordered);
}
