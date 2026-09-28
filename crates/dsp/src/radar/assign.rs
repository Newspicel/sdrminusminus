use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AssignError {
    #[error("assignment holds {max_rows} x {max_cols}, asked for {rows} x {cols}")]
    Capacity {
        rows: usize,
        cols: usize,
        max_rows: usize,
        max_cols: usize,
    },
    #[error("assignment output holds {held} rows, needs {rows}")]
    Output { held: usize, rows: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
struct Cost {
    forbidden: f64,
    total: f64,
}

impl Cost {
    const OPEN: Self = Self {
        forbidden: 0.0,
        total: 0.0,
    };
    const FORBIDDEN: Self = Self {
        forbidden: 1.0,
        total: 0.0,
    };
    const UNBOUNDED: Self = Self {
        forbidden: f64::INFINITY,
        total: f64::INFINITY,
    };

    fn of(value: Option<f64>) -> Self {
        match value {
            Some(total) if total.is_finite() => Self {
                forbidden: 0.0,
                total,
            },
            _ => Self::FORBIDDEN,
        }
    }

    fn allowed(self) -> bool {
        self.forbidden == 0.0
    }
}

impl Add for Cost {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            forbidden: self.forbidden + other.forbidden,
            total: self.total + other.total,
        }
    }
}

impl Sub for Cost {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            forbidden: self.forbidden - other.forbidden,
            total: self.total - other.total,
        }
    }
}

impl AddAssign for Cost {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl SubAssign for Cost {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

pub struct Assignment {
    max_rows: usize,
    max_cols: usize,
    cost: Vec<Cost>,
    row_potential: Vec<Cost>,
    col_potential: Vec<Cost>,
    col_owner: Vec<usize>,
    way: Vec<usize>,
    slack: Vec<Cost>,
    used: Vec<bool>,
}

impl Assignment {
    #[must_use]
    pub fn new(max_rows: usize, max_cols: usize) -> Self {
        let size = max_rows.max(max_cols);
        Self {
            max_rows,
            max_cols,
            cost: vec![Cost::OPEN; size * size],
            row_potential: vec![Cost::OPEN; size + 1],
            col_potential: vec![Cost::OPEN; size + 1],
            col_owner: vec![0; size + 1],
            way: vec![0; size + 1],
            slack: vec![Cost::OPEN; size + 1],
            used: vec![false; size + 1],
        }
    }

    pub fn solve(
        &mut self,
        rows: usize,
        cols: usize,
        cost: impl Fn(usize, usize) -> Option<f64>,
        out: &mut [Option<usize>],
    ) -> Result<usize, AssignError> {
        if rows > self.max_rows || cols > self.max_cols {
            return Err(AssignError::Capacity {
                rows,
                cols,
                max_rows: self.max_rows,
                max_cols: self.max_cols,
            });
        }
        if out.len() < rows {
            return Err(AssignError::Output {
                held: out.len(),
                rows,
            });
        }
        out[..rows].fill(None);
        let size = rows.max(cols);
        if rows == 0 || cols == 0 {
            return Ok(0);
        }
        self.fill(rows, cols, size, cost);
        self.run(size);
        Ok(self.collect(rows, cols, size, out))
    }

    fn fill(
        &mut self,
        rows: usize,
        cols: usize,
        size: usize,
        cost: impl Fn(usize, usize) -> Option<f64>,
    ) {
        for row in 0..size {
            for col in 0..size {
                self.cost[row * size + col] = if row < rows && col < cols {
                    Cost::of(cost(row, col))
                } else {
                    Cost::OPEN
                };
            }
        }
    }

    fn run(&mut self, size: usize) {
        self.row_potential[..=size].fill(Cost::OPEN);
        self.col_potential[..=size].fill(Cost::OPEN);
        self.col_owner[..=size].fill(0);
        self.way[..=size].fill(0);
        for row in 1..=size {
            self.col_owner[0] = row;
            self.slack[..=size].fill(Cost::UNBOUNDED);
            self.used[..=size].fill(false);
            let free = self.search(size);
            self.augment(free);
        }
    }

    fn search(&mut self, size: usize) -> usize {
        let mut current = 0;
        for _ in 0..=size {
            self.used[current] = true;
            let owner = self.col_owner[current];
            let mut delta = Cost::UNBOUNDED;
            let mut next = 0;
            for col in 1..=size {
                if self.used[col] {
                    continue;
                }
                let reduced = self.cost[(owner - 1) * size + col - 1]
                    - self.row_potential[owner]
                    - self.col_potential[col];
                if reduced < self.slack[col] {
                    self.slack[col] = reduced;
                    self.way[col] = current;
                }
                if self.slack[col] < delta {
                    delta = self.slack[col];
                    next = col;
                }
            }
            for col in 0..=size {
                if self.used[col] {
                    self.row_potential[self.col_owner[col]] += delta;
                    self.col_potential[col] -= delta;
                } else {
                    self.slack[col] -= delta;
                }
            }
            current = next;
            if self.col_owner[current] == 0 {
                break;
            }
        }
        current
    }

    fn augment(&mut self, free: usize) {
        let mut current = free;
        while current != 0 {
            let previous = self.way[current];
            self.col_owner[current] = self.col_owner[previous];
            current = previous;
        }
    }

    fn collect(&self, rows: usize, cols: usize, size: usize, out: &mut [Option<usize>]) -> usize {
        let mut assigned = 0;
        for col in 1..=cols {
            let row = self.col_owner[col];
            if row == 0 || row > rows {
                continue;
            }
            if self.cost[(row - 1) * size + col - 1].allowed() {
                out[row - 1] = Some(col - 1);
                assigned += 1;
            }
        }
        assigned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    type Table = Vec<Vec<Option<f64>>>;

    fn random_table(rng: &mut Rng, rows: usize, cols: usize, forbid: f64) -> Table {
        (0..rows)
            .map(|_| {
                (0..cols)
                    .map(|_| {
                        let value = rng.next() * 20.0 - 5.0;
                        (rng.next() >= forbid).then_some(value)
                    })
                    .collect()
            })
            .collect()
    }

    fn score(table: &Table, pick: &[Option<usize>]) -> (usize, f64) {
        let mut matched = 0;
        let mut total = 0.0;
        for (row, col) in pick.iter().enumerate() {
            if let Some(col) = col {
                let value = table[row][*col].unwrap();
                matched += 1;
                total += value;
            }
        }
        (matched, total)
    }

    fn brute_force(table: &Table, cols: usize) -> (usize, f64) {
        fn walk(
            table: &Table,
            row: usize,
            taken: &mut Vec<bool>,
            matched: usize,
            total: f64,
            best: &mut (usize, f64),
        ) {
            if row == table.len() {
                if matched > best.0 || (matched == best.0 && total < best.1) {
                    *best = (matched, total);
                }
                return;
            }
            walk(table, row + 1, taken, matched, total, best);
            for col in 0..taken.len() {
                if let (false, Some(value)) = (taken[col], table[row][col]) {
                    taken[col] = true;
                    walk(table, row + 1, taken, matched + 1, total + value, best);
                    taken[col] = false;
                }
            }
        }
        let mut best = (0, 0.0);
        walk(table, 0, &mut vec![false; cols], 0, 0.0, &mut best);
        best
    }

    fn solve(assignment: &mut Assignment, table: &Table, cols: usize) -> Vec<Option<usize>> {
        let mut out = vec![Some(usize::MAX); table.len()];
        let count = assignment
            .solve(table.len(), cols, |row, col| table[row][col], &mut out)
            .unwrap();
        assert_eq!(count, out.iter().flatten().count());
        out
    }

    fn assert_is_matching(pick: &[Option<usize>], table: &Table) {
        let mut seen = std::collections::HashSet::new();
        for (row, col) in pick.iter().enumerate() {
            if let Some(col) = col {
                assert!(seen.insert(*col), "column {col} used twice");
                assert!(table[row][*col].is_some(), "forbidden pair {row}, {col}");
            }
        }
    }

    #[test]
    fn hungarian_matches_brute_force() {
        let mut rng = Rng(0x1234_5678_9abc_def1);
        let mut assignment = Assignment::new(6, 6);
        for case in 0..200 {
            let forbid = [0.0, 0.2, 0.5][case % 3];
            let table = random_table(&mut rng, 6, 6, forbid);
            let pick = solve(&mut assignment, &table, 6);
            assert_is_matching(&pick, &table);
            let (matched, total) = score(&table, &pick);
            let (best_matched, best_total) = brute_force(&table, 6);
            assert_eq!(matched, best_matched, "case {case}");
            assert!(
                (total - best_total).abs() < 1e-9,
                "case {case}: {total} vs {best_total}"
            );
        }
    }

    #[test]
    fn forbidden_pairs_stay_unassigned() {
        let mut assignment = Assignment::new(3, 3);
        let table = vec![
            vec![Some(1.0), None, None],
            vec![None, None, None],
            vec![Some(5.0), None, Some(f64::NAN)],
        ];
        let pick = solve(&mut assignment, &table, 3);
        assert_eq!(pick, vec![Some(0), None, None]);

        let crossing = vec![vec![Some(1.0), Some(100.0)], vec![Some(2.0), None]];
        let pick = solve(&mut assignment, &crossing, 2);
        assert_eq!(pick, vec![Some(1), Some(0)]);
    }

    #[test]
    fn rectangular_problems_are_solved() {
        let mut rng = Rng(0x0dd_ba11_cafe_f00d);
        let mut assignment = Assignment::new(7, 7);
        for (rows, cols) in [(3, 5), (5, 3), (1, 7), (7, 1), (4, 6), (6, 2)] {
            for case in 0..40 {
                let table = random_table(&mut rng, rows, cols, 0.25);
                let pick = solve(&mut assignment, &table, cols);
                assert_eq!(pick.len(), rows);
                assert_is_matching(&pick, &table);
                let (matched, total) = score(&table, &pick);
                let (best_matched, best_total) = brute_force(&table, cols);
                assert!(matched <= rows.min(cols));
                assert_eq!(matched, best_matched, "{rows} x {cols}, case {case}");
                assert!(
                    (total - best_total).abs() < 1e-9,
                    "{rows} x {cols}, case {case}: {total} vs {best_total}"
                );
            }
        }
    }

    #[test]
    fn empty_problems_assign_nothing() {
        let mut assignment = Assignment::new(4, 4);
        let mut out = [Some(3); 4];
        assert_eq!(assignment.solve(4, 0, |_, _| Some(1.0), &mut out), Ok(0));
        assert_eq!(out, [None; 4]);
        assert_eq!(assignment.solve(0, 4, |_, _| Some(1.0), &mut out), Ok(0));
    }

    #[test]
    fn oversized_problems_are_refused() {
        let mut assignment = Assignment::new(2, 3);
        let mut out = [None; 4];
        assert_eq!(
            assignment.solve(3, 3, |_, _| Some(1.0), &mut out),
            Err(AssignError::Capacity {
                rows: 3,
                cols: 3,
                max_rows: 2,
                max_cols: 3
            })
        );
        assert_eq!(
            assignment.solve(2, 3, |_, _| Some(1.0), &mut out[..1]),
            Err(AssignError::Output { held: 1, rows: 2 })
        );
    }
}
