use std::sync::Mutex;

use knead_net::input::pcb::parse_pcb;
use knead_net::{
    Breadboard, BridgeInitial, BridgePolicy, Layout, LayoutProgress, PathFinderRouter, Preset,
    ProgressOptions, SAConfig, prepare_for_layout,
};

const FIXED_SEEDS: [u64; 8] = [
    0x514A_0000,
    0x514A_0001,
    0x514A_0002,
    0x514A_0003,
    0x514A_0004,
    0x514A_0005,
    0x514A_0006,
    0x514A_0007,
];

#[derive(Debug, Clone, Copy)]
struct Fixture {
    name: &'static str,
    source: &'static str,
    spectral_envelope: QualityEnvelope,
}

#[derive(Debug, Clone, Copy)]
struct QualityEnvelope {
    initial_cost_median: f64,
    final_cost_median: f64,
    wire_count_median: f64,
    wire_length_median: f64,
}

const FIXTURES: [Fixture; 3] = [
    Fixture {
        name: "h-bridge",
        source: include_str!("../examples/inputs/h-bridge.kicad_pcb"),
        spectral_envelope: QualityEnvelope {
            initial_cost_median: 1_020.0,
            final_cost_median: 750.0,
            wire_count_median: 19.0,
            wire_length_median: 95.0,
        },
    },
    Fixture {
        name: "lm741",
        source: include_str!("../examples/inputs/lm741.kicad_pcb"),
        spectral_envelope: QualityEnvelope {
            initial_cost_median: 700.0,
            final_cost_median: 350.0,
            wire_count_median: 10.0,
            wire_length_median: 55.0,
        },
    },
    Fixture {
        name: "SNx4HC00",
        source: include_str!("../examples/inputs/SNx4HC00.kicad_pcb"),
        spectral_envelope: QualityEnvelope {
            initial_cost_median: 750.0,
            final_cost_median: 550.0,
            wire_count_median: 18.0,
            wire_length_median: 70.0,
        },
    },
];

#[derive(Debug, Clone, Copy)]
enum Initializer {
    Greedy,
    Spectral,
}

impl Initializer {
    fn name(self) -> &'static str {
        match self {
            Self::Greedy => "greedy",
            Self::Spectral => "spectral",
        }
    }

    fn use_spectral(self) -> bool {
        matches!(self, Self::Spectral)
    }
}

#[derive(Debug, Clone, Copy)]
struct RunMetrics {
    initial_cost: f64,
    final_cost: f64,
    wire_count: usize,
    wire_length: usize,
}

#[derive(Debug, Clone, Copy)]
struct Summary {
    min: f64,
    median: f64,
    mean: f64,
    max: f64,
}

impl Summary {
    fn of(values: impl IntoIterator<Item = f64>) -> Self {
        let mut values: Vec<f64> = values.into_iter().collect();
        assert!(
            !values.is_empty(),
            "quality summary requires at least one sample"
        );
        values.sort_by(f64::total_cmp);
        Self {
            min: values[0],
            median: values[values.len() / 2],
            mean: values.iter().sum::<f64>() / values.len() as f64,
            max: values[values.len() - 1],
        }
    }
}

fn quick_config(seed: u64, initializer: Initializer, max_iters: usize) -> SAConfig {
    SAConfig {
        max_iters,
        t_start: 40.0,
        t_end: 0.1,
        seed,
        n_seeds: 1,
        use_spectral: initializer.use_spectral(),
        bridge_policy: BridgePolicy::Explore {
            initial: BridgeInitial::BestOfBoth,
        },
        ..SAConfig::default()
    }
}

fn run_fixture(
    fixture: Fixture,
    initializer: Initializer,
    seed: u64,
    max_iters: usize,
) -> RunMetrics {
    let mut circuit = parse_pcb(fixture.source).expect("quality fixture should parse");
    let prepared = prepare_for_layout(&mut circuit, Preset::Hole800.make(63));
    let board = prepared.board;
    let config = quick_config(seed, initializer, max_iters);
    let progress_costs = Mutex::new((None, None));
    let mut layout = Layout::new(&circuit);

    layout
        .place_sa_with_progress(
            &board,
            &config,
            ProgressOptions {
                display_seed: 0,
                sample_every: usize::MAX,
            },
            |event| {
                let mut costs = progress_costs.lock().expect("quality metrics lock");
                match event {
                    LayoutProgress::Annealing {
                        iteration: 0,
                        current_cost,
                        ..
                    } => costs.0 = Some(current_cost),
                    LayoutProgress::PlacementComplete { cost, .. } => costs.1 = Some(cost),
                    _ => {}
                }
            },
        )
        .unwrap_or_else(|errors| {
            panic!(
                "{} {} seed {seed:#x} placement failed: {errors:?}",
                fixture.name,
                initializer.name(),
            )
        });

    let (initial_cost, final_cost) = *progress_costs.lock().expect("quality metrics lock");
    let initial_cost = initial_cost.expect("iteration zero should report post-bridge initial cost");
    let final_cost = final_cost.expect("placement completion should report final cost");
    assert!(
        final_cost <= initial_cost,
        "{} {} seed {seed:#x} worsened: initial={initial_cost}, final={final_cost}",
        fixture.name,
        initializer.name(),
    );

    layout
        .route_with_progress(&board, &PathFinderRouter::default(), |_| {})
        .unwrap_or_else(|errors| {
            panic!(
                "{} {} seed {seed:#x} routing failed: {errors:?}",
                fixture.name,
                initializer.name(),
            )
        });
    layout.validate(&board).unwrap_or_else(|errors| {
        panic!(
            "{} {} seed {seed:#x} validation failed: {errors:?}",
            fixture.name,
            initializer.name(),
        )
    });

    RunMetrics {
        initial_cost,
        final_cost,
        wire_count: layout.wires().len(),
        wire_length: total_wire_length(layout.wires(), &board),
    }
}

fn total_wire_length(wires: &[knead_net::Wire], board: &Breadboard) -> usize {
    wires
        .iter()
        .map(|wire| {
            let from = board.hole(wire.from).position;
            let to = board.hole(wire.to).position;
            from.x.abs_diff(to.x) as usize + from.y.abs_diff(to.y) as usize
        })
        .sum()
}

fn print_summary(fixture: Fixture, initializer: Initializer, runs: &[RunMetrics]) {
    let initial = Summary::of(runs.iter().map(|run| run.initial_cost));
    let final_cost = Summary::of(runs.iter().map(|run| run.final_cost));
    let wires = Summary::of(runs.iter().map(|run| run.wire_count as f64));
    let wire_length = Summary::of(runs.iter().map(|run| run.wire_length as f64));
    eprintln!(
        "QUALITY fixture={} initializer={} samples={} initial={} final={} wires={} wire_length={}",
        fixture.name,
        initializer.name(),
        runs.len(),
        format_summary(initial),
        format_summary(final_cost),
        format_summary(wires),
        format_summary(wire_length),
    );

    if matches!(initializer, Initializer::Spectral) {
        assert_quality_envelope(fixture, initial, final_cost, wires, wire_length);
    }
}

fn assert_quality_envelope(
    fixture: Fixture,
    initial: Summary,
    final_cost: Summary,
    wires: Summary,
    wire_length: Summary,
) {
    let envelope = fixture.spectral_envelope;
    for (metric, actual, maximum) in [
        (
            "initial cost median",
            initial.median,
            envelope.initial_cost_median,
        ),
        (
            "final cost median",
            final_cost.median,
            envelope.final_cost_median,
        ),
        (
            "wire count median",
            wires.median,
            envelope.wire_count_median,
        ),
        (
            "wire length median",
            wire_length.median,
            envelope.wire_length_median,
        ),
    ] {
        assert!(
            actual <= maximum,
            "{} spectral {metric} regressed: actual={actual}, maximum={maximum}",
            fixture.name,
        );
    }
}

fn format_summary(summary: Summary) -> String {
    format!(
        "[min={:.3},median={:.3},mean={:.3},max={:.3}]",
        summary.min, summary.median, summary.mean, summary.max,
    )
}

#[test]
fn quality_probe_captures_post_bridge_cost_and_routing_metrics() {
    let metrics = run_fixture(FIXTURES[0], Initializer::Spectral, FIXED_SEEDS[0], 32);
    assert!(metrics.initial_cost.is_finite());
    assert!(metrics.final_cost.is_finite());
    assert!(metrics.wire_count > 0);
    assert!(metrics.wire_length > 0);
}

#[test]
#[ignore = "fixed-seed quality report; run explicitly before initializer changes"]
fn report_initializer_quality() {
    for fixture in FIXTURES {
        for initializer in [Initializer::Greedy, Initializer::Spectral] {
            let runs: Vec<RunMetrics> = FIXED_SEEDS
                .into_iter()
                .map(|seed| run_fixture(fixture, initializer, seed, 5_000))
                .collect();
            print_summary(fixture, initializer, &runs);
        }
    }
}
