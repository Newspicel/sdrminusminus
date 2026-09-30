use sdrmm_dsp::radar::assign::Assignment;
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

fn cost(row: usize, col: usize) -> Option<f64> {
    let spread = row.abs_diff(col) as f64;
    (spread < 5.0).then_some(spread * 1.5 + (row * 7 + col * 3) as f64 % 4.0)
}

#[test]
fn assignment_does_not_allocate() {
    let mut assignment = Assignment::new(24, 32);
    let mut out = [None; 24];
    assert!(assignment.solve(24, 32, cost, &mut out).is_ok());
    assert_no_alloc("assignment", || {
        for (rows, cols) in [(24, 32), (1, 1), (17, 5), (24, 24), (0, 8)] {
            let assigned = assignment.solve(rows, cols, cost, &mut out);
            assert!(assigned.is_ok_and(|count| count <= rows.min(cols)));
        }
    });
}
