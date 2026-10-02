use std::sync::Arc;

use super::*;

fn value(shape: &[usize]) -> Value {
    Value {
        shape: shape.to_vec(),
        data: None,
    }
}

fn weights(shape: &[usize], data: &[f32]) -> Value {
    Value {
        shape: shape.to_vec(),
        data: Some(Weights::F32(data.to_vec())),
    }
}

fn graph(values: Vec<Value>, nodes: Vec<Node>, inputs: Vec<usize>, outputs: Vec<usize>) -> Graph {
    Graph {
        meta: vec![("hop_length".into(), "160".into())],
        values,
        nodes,
        inputs,
        outputs,
    }
}

fn node(op: Op, inputs: &[usize], outputs: &[usize]) -> Node {
    Node {
        op,
        inputs: inputs.to_vec(),
        outputs: outputs.to_vec(),
    }
}

fn run(graph: &Graph, input: &[f32]) -> Vec<f32> {
    let net = Arc::new(Net::from_graph(graph).expect("compiles"));
    let mut session = Session::new(net);
    session.input_mut(0).copy_from_slice(input);
    session.run();
    session.output(0).to_vec()
}

#[test]
fn half_precision_round_trips_representable_values() {
    for value in [
        0.0f32,
        -0.0,
        1.0,
        -2.5,
        65504.0,
        6.1035156e-5,
        5.9604645e-8,
        0.333_251_95,
    ] {
        assert_eq!(
            f16_to_f32(f32_to_f16(value)).to_bits(),
            value.to_bits(),
            "{value}"
        );
    }
    assert_eq!(f32_to_f16(1.0e6), 0x7c00);
    assert!(f16_to_f32(f32_to_f16(f32::NAN)).is_nan());
    assert_eq!(f16_to_f32(f32_to_f16(1.0 + 1.0 / 4096.0)), 1.0);
}

#[test]
fn a_graph_survives_encoding() {
    let g = graph(
        vec![
            value(&[2, 3]),
            Value {
                shape: vec![3],
                data: Some(Weights::F16(vec![0x3c00, 0x4000, 0x4200])),
            },
            value(&[2, 3]),
        ],
        vec![
            node(Op::Binary(Binary::Mul), &[0, 1], &[2]),
            node(
                Op::Conv(ConvSpec {
                    channels_last: true,
                    batched: false,
                    group: 2,
                    strides: vec![1],
                    dilations: vec![2],
                    pads_before: vec![1],
                    pads_after: vec![0],
                }),
                &[0, 1, 1],
                &[2],
            ),
        ],
        vec![0],
        vec![2],
    );
    assert_eq!(Graph::decode(&g.encode()).expect("decodes"), g);
}

#[test]
fn a_cut_file_is_refused() {
    let g = graph(vec![value(&[1])], vec![], vec![0], vec![0]);
    let bytes = g.encode();
    assert!(matches!(
        Graph::decode(&bytes[..bytes.len() - 1]),
        Err(NetError::Format(_))
    ));
    assert!(matches!(
        Graph::decode(b"garbage!"),
        Err(NetError::Format(_))
    ));
}

#[test]
fn broadcasting_matches_numpy() {
    let g = graph(
        vec![
            value(&[2, 3]),
            weights(&[1, 3], &[10.0, 20.0, 30.0]),
            value(&[2, 3]),
        ],
        vec![node(Op::Binary(Binary::Sub), &[0, 1], &[2])],
        vec![0],
        vec![2],
    );
    assert_eq!(
        run(&g, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        [-9.0, -18.0, -27.0, -6.0, -15.0, -24.0]
    );
}

#[test]
fn transpose_slice_and_concat_move_the_right_numbers() {
    let g = graph(
        vec![
            value(&[2, 3]),
            value(&[3, 2]),
            value(&[1, 2]),
            value(&[4, 2]),
        ],
        vec![
            node(Op::Transpose { perm: vec![1, 0] }, &[0], &[1]),
            node(
                Op::Slice {
                    axis: 0,
                    start: 2,
                    end: 3,
                },
                &[1],
                &[2],
            ),
            node(Op::Concat { axis: 0 }, &[1, 2], &[3]),
        ],
        vec![0],
        vec![3],
    );
    assert_eq!(
        run(&g, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        [1.0, 4.0, 2.0, 5.0, 3.0, 6.0, 3.0, 6.0]
    );
}

#[test]
fn reduce_and_rms_norm_follow_their_definitions() {
    let g = graph(
        vec![value(&[2, 2]), value(&[2, 1]), value(&[2, 2])],
        vec![
            node(Op::SumReduce { axes: vec![1] }, &[0], &[1]),
            node(Op::RmsNorm { axis: 1, eps: 0.0 }, &[0], &[2]),
        ],
        vec![0],
        vec![1, 2],
    );
    let net = Arc::new(Net::from_graph(&g).expect("compiles"));
    let mut session = Session::new(net);
    session.input_mut(0).copy_from_slice(&[3.0, 4.0, -1.0, 1.0]);
    session.run();
    assert_eq!(session.output(0), [7.0, 0.0]);
    let norm = (12.5f32).sqrt();
    let expected = [3.0 / norm, 4.0 / norm, -1.0, 1.0];
    for (got, want) in session.output(1).iter().zip(expected) {
        assert!((got - want).abs() < 1e-6);
    }
}

#[test]
fn einsum_covers_every_loop_order() {
    let b = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    for (labels, b_shape, expected) in [
        (b"nk".to_vec(), [3, 2], [5.0, 11.0, 17.0, 11.0, 25.0, 39.0]),
        (b"kn".to_vec(), [2, 3], [9.0, 12.0, 15.0, 19.0, 26.0, 33.0]),
    ] {
        let g = graph(
            vec![value(&[2, 2]), weights(&b_shape, &b), value(&[2, 3])],
            vec![node(
                Op::EinSum {
                    a: b"mk".to_vec(),
                    b: labels,
                    out: b"mn".to_vec(),
                },
                &[0, 1],
                &[2],
            )],
            vec![0],
            vec![2],
        );
        assert_eq!(run(&g, &[1.0, 2.0, 3.0, 4.0]), expected);
    }
    let g = graph(
        vec![
            value(&[2, 3]),
            weights(&[2, 2], &[1.0, 2.0, 3.0, 4.0]),
            value(&[2, 3]),
        ],
        vec![node(
            Op::EinSum {
                a: b"km".to_vec(),
                b: b"nk".to_vec(),
                out: b"nm".to_vec(),
            },
            &[0, 1],
            &[2],
        )],
        vec![0],
        vec![2],
    );
    assert_eq!(
        run(&g, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        [9.0, 12.0, 15.0, 19.0, 26.0, 33.0]
    );
}

#[test]
fn conv_pads_strides_and_groups() {
    let g = graph(
        vec![
            value(&[1, 2, 1, 4]),
            weights(&[2, 1, 1, 3], &[1.0, 0.0, -1.0, 1.0, 1.0, 1.0]),
            weights(&[2], &[0.5, -0.5]),
            value(&[1, 2, 1, 2]),
        ],
        vec![node(
            Op::Conv(ConvSpec {
                channels_last: false,
                batched: true,
                group: 2,
                strides: vec![1, 2],
                dilations: vec![1, 1],
                pads_before: vec![0, 1],
                pads_after: vec![0, 0],
            }),
            &[0, 1, 2],
            &[3],
        )],
        vec![0],
        vec![3],
    );
    assert_eq!(
        run(&g, &[1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0]),
        [-1.5, -1.5, 29.5, 89.5]
    );
}

#[test]
fn gru_runs_both_ways_from_its_initial_state() {
    let hidden = 1;
    let zeros = [0.0; 3];
    for (backward, expected) in [
        (false, [0.5f32.tanh() * 0.5]),
        (true, [0.5f32.tanh() * 0.5]),
    ] {
        let g = graph(
            vec![
                value(&[1, 1, 1]),
                weights(&[3, 1], &[0.0, 0.0, 1.0]),
                weights(&[3, 1], &zeros),
                weights(&[1, 6], &[0.0; 6]),
                weights(&[1, 1, 1], &[0.0]),
                value(&[1, 1, 1]),
                value(&[1, 1, 1]),
            ],
            vec![node(
                Op::Gru { hidden, backward },
                &[0, 1, 2, 3, 4],
                &[5, 6],
            )],
            vec![0],
            vec![5],
        );
        let got = run(&g, &[0.5]);
        assert!((got[0] - expected[0]).abs() < 1e-6, "{got:?}");
    }
}

#[test]
fn gather_and_reflect_pad_index_correctly() {
    let g = graph(
        vec![value(&[1, 4]), value(&[1, 2]), value(&[1, 6])],
        vec![
            node(
                Op::Gather {
                    axis: 1,
                    indices: vec![3, 0],
                },
                &[0],
                &[1],
            ),
            node(
                Op::PadReflect {
                    before: vec![0, 1],
                    after: vec![0, 1],
                },
                &[0],
                &[2],
            ),
        ],
        vec![0],
        vec![1, 2],
    );
    let net = Arc::new(Net::from_graph(&g).expect("compiles"));
    let mut session = Session::new(net);
    session.input_mut(0).copy_from_slice(&[1.0, 2.0, 3.0, 4.0]);
    session.run();
    assert_eq!(session.output(0), [4.0, 1.0]);
    assert_eq!(session.output(1), [2.0, 1.0, 2.0, 3.0, 4.0, 3.0]);
}

#[test]
fn inconsistent_shapes_are_refused_before_running() {
    let g = graph(
        vec![value(&[2, 3]), value(&[3, 3])],
        vec![node(Op::Unary(Unary::Tanh), &[0], &[1])],
        vec![0],
        vec![1],
    );
    assert!(matches!(Net::from_graph(&g), Err(NetError::Unsupported(_))));
    let g = graph(
        vec![value(&[4]), value(&[2])],
        vec![node(
            Op::Slice {
                axis: 0,
                start: 3,
                end: 5,
            },
            &[0],
            &[1],
        )],
        vec![0],
        vec![1],
    );
    assert!(Net::from_graph(&g).is_err());
}
