use std::collections::HashMap;

use knead_net::input::pcb::parse_pcb;
use knead_net::{
    BridgeInitial, BridgePolicy, HoleId, Layout, NetId, PathFinderRouter, Placement, Preset,
    SAConfig, prepare_for_layout,
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
    let prepared = prepare_for_layout(&mut circuit, Preset::Hole800.make(63));
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
