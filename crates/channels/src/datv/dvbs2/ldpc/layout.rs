use super::GROUP;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Edge {
    pub base: usize,
    pub shift: usize,
    pub open: bool,
}

impl Edge {
    #[cfg(any(test, feature = "synth"))]
    pub(super) const fn position(self, lane: usize) -> Option<usize> {
        if self.open && lane < GROUP - self.shift {
            None
        } else {
            Some(self.base + (lane + self.shift) % GROUP)
        }
    }
}

pub(super) struct Layout {
    pub information: usize,
    pub layers: usize,
    pub edges: Vec<Edge>,
    pub bounds: Vec<usize>,
    pub degree: usize,
}

impl Layout {
    pub(super) fn build(length: usize, addresses: &[&[u16]]) -> Option<Self> {
        let information = addresses.len() * GROUP;
        let parity = length.checked_sub(information)?;
        let layers = parity / GROUP;
        if information == 0 || layers < 2 || !parity.is_multiple_of(GROUP) {
            return None;
        }
        if addresses
            .iter()
            .any(|row| row.is_empty() || row.iter().any(|&address| usize::from(address) >= parity))
        {
            return None;
        }
        let mut grouped: Vec<Vec<Edge>> = vec![Vec::new(); layers];
        for (group, row) in addresses.iter().enumerate() {
            for &address in *row {
                let address = usize::from(address);
                grouped[address % layers].push(Edge {
                    base: group * GROUP,
                    shift: (GROUP - address / layers) % GROUP,
                    open: false,
                });
            }
        }
        for (layer, edges) in grouped.iter_mut().enumerate() {
            edges.extend(staircase(information, layers, layer));
        }
        Some(Self::flatten(information, grouped))
    }

    fn flatten(information: usize, grouped: Vec<Vec<Edge>>) -> Self {
        let layers = grouped.len();
        let degree = grouped.iter().map(Vec::len).max().unwrap_or(0);
        let mut bounds = Vec::with_capacity(layers + 1);
        bounds.push(0);
        let mut edges = Vec::new();
        for layer in grouped {
            edges.extend(layer);
            bounds.push(edges.len());
        }
        Self {
            information,
            layers,
            edges,
            bounds,
            degree,
        }
    }

    pub(super) fn layer(&self, layer: usize) -> &[Edge] {
        &self.edges[self.bounds[layer]..self.bounds[layer + 1]]
    }

    #[cfg(any(test, feature = "synth"))]
    pub(super) const fn check(&self, layer: usize, lane: usize) -> usize {
        layer + self.layers * lane
    }

    pub(super) const fn position(&self, bit: usize) -> usize {
        if bit < self.information {
            bit
        } else {
            let parity = bit - self.information;
            self.information + (parity % self.layers) * GROUP + parity / self.layers
        }
    }
}

fn staircase(information: usize, layers: usize, layer: usize) -> [Edge; 2] {
    let own = information + layer * GROUP;
    let previous = if layer == 0 {
        Edge {
            base: information + (layers - 1) * GROUP,
            shift: GROUP - 1,
            open: true,
        }
    } else {
        Edge {
            base: own - GROUP,
            shift: 0,
            open: false,
        }
    };
    [
        Edge {
            base: own,
            shift: 0,
            open: false,
        },
        previous,
    ]
}
