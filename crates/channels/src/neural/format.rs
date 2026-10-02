use super::NetError;

const MAGIC: &[u8; 8] = b"SDRMMNN1";

#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    pub meta: Vec<(String, String)>,
    pub values: Vec<Value>,
    pub nodes: Vec<Node>,
    pub inputs: Vec<usize>,
    pub outputs: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    pub shape: Vec<usize>,
    pub data: Option<Weights>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Weights {
    F32(Vec<f32>),
    F16(Vec<u16>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub op: Op,
    pub inputs: Vec<usize>,
    pub outputs: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unary {
    Sqrt,
    Rsqrt,
    Square,
    Ln,
    Sigmoid,
    Tanh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binary {
    Add,
    Sub,
    Mul,
    Max,
    Min,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConvSpec {
    pub channels_last: bool,
    pub batched: bool,
    pub group: usize,
    pub strides: Vec<usize>,
    pub dilations: Vec<usize>,
    pub pads_before: Vec<usize>,
    pub pads_after: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Alias,
    Unary(Unary),
    Binary(Binary),
    SumReduce {
        axes: Vec<usize>,
    },
    Slice {
        axis: usize,
        start: usize,
        end: usize,
    },
    Concat {
        axis: usize,
    },
    Transpose {
        perm: Vec<usize>,
    },
    EinSum {
        a: Vec<u8>,
        b: Vec<u8>,
        out: Vec<u8>,
    },
    Conv(ConvSpec),
    Gru {
        hidden: usize,
        backward: bool,
    },
    RmsNorm {
        axis: usize,
        eps: f32,
    },
    Gather {
        axis: usize,
        indices: Vec<usize>,
    },
    PadReflect {
        before: Vec<usize>,
        after: Vec<usize>,
    },
}

impl Graph {
    #[must_use]
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer(MAGIC.to_vec());
        out.len(self.meta.len());
        for (key, value) in &self.meta {
            out.text(key);
            out.text(value);
        }
        out.len(self.values.len());
        for value in &self.values {
            out.list(&value.shape);
            match &value.data {
                None => out.byte(0),
                Some(Weights::F32(data)) => {
                    out.byte(1);
                    out.len(data.len());
                    data.iter().for_each(|&x| out.0.extend(x.to_le_bytes()));
                }
                Some(Weights::F16(data)) => {
                    out.byte(2);
                    out.len(data.len());
                    data.iter().for_each(|&x| out.0.extend(x.to_le_bytes()));
                }
            }
        }
        out.len(self.nodes.len());
        for node in &self.nodes {
            out.op(&node.op);
            out.list(&node.inputs);
            out.list(&node.outputs);
        }
        out.list(&self.inputs);
        out.list(&self.outputs);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, NetError> {
        let mut input = Reader { bytes, at: 0 };
        if input.take(MAGIC.len())? != MAGIC {
            return Err(NetError::Format("not an sdrmm network".into()));
        }
        let meta = (0..input.len()?)
            .map(|_| Ok((input.text()?, input.text()?)))
            .collect::<Result<_, NetError>>()?;
        let values = (0..input.len()?)
            .map(|_| input.value())
            .collect::<Result<_, _>>()?;
        let nodes = (0..input.len()?)
            .map(|_| {
                Ok(Node {
                    op: input.op()?,
                    inputs: input.list()?,
                    outputs: input.list()?,
                })
            })
            .collect::<Result<_, NetError>>()?;
        let graph = Self {
            meta,
            values,
            nodes,
            inputs: input.list()?,
            outputs: input.list()?,
        };
        if input.at != bytes.len() {
            return Err(NetError::Format("trailing bytes".into()));
        }
        Ok(graph)
    }
}

impl Weights {
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::F32(data) => data.len(),
            Self::F16(data) => data.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn to_f32(&self) -> Vec<f32> {
        match self {
            Self::F32(data) => data.clone(),
            Self::F16(data) => data.iter().map(|&bits| f16_to_f32(bits)).collect(),
        }
    }
}

#[must_use]
pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = u32::from(bits >> 10) & 0x1f;
    let mantissa = u32::from(bits & 0x3ff);
    let magnitude = match exponent {
        0 => return subnormal_f16(mantissa, sign),
        0x1f => 0x7f80_0000 | (mantissa << 13),
        _ => ((exponent + 112) << 23) | (mantissa << 13),
    };
    f32::from_bits(sign | magnitude)
}

fn subnormal_f16(mantissa: u32, sign: u32) -> f32 {
    let value = mantissa as f32 * 2f32.powi(-24);
    if sign == 0 { value } else { -value }
}

#[must_use]
pub fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x7f_ffff;
    if exponent == 0xff {
        let nan = if mantissa == 0 { 0 } else { 0x200 };
        return sign | 0x7c00 | nan;
    }
    let unbiased = exponent - 127;
    if unbiased > 15 {
        return sign | 0x7c00;
    }
    if unbiased >= -14 {
        let half = (((unbiased + 15) as u32) << 10) | (mantissa >> 13);
        return sign | round_even(half, mantissa & 0x1fff, 0x1000) as u16;
    }
    if unbiased < -25 {
        return sign;
    }
    let full = mantissa | 0x80_0000;
    let shift = (-unbiased - 1) as u32;
    let half = full >> shift;
    let rest = full & ((1 << shift) - 1);
    sign | round_even(half, rest, 1 << (shift - 1)) as u16
}

fn round_even(value: u32, rest: u32, halfway: u32) -> u32 {
    if rest > halfway || (rest == halfway && value & 1 == 1) {
        value + 1
    } else {
        value
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn byte(&mut self, value: u8) {
        self.0.push(value);
    }

    fn len(&mut self, value: usize) {
        self.0.extend((value as u64).to_le_bytes());
    }

    fn float(&mut self, value: f32) {
        self.0.extend(value.to_le_bytes());
    }

    fn text(&mut self, value: &str) {
        self.len(value.len());
        self.0.extend(value.as_bytes());
    }

    fn list(&mut self, values: &[usize]) {
        self.len(values.len());
        values.iter().for_each(|&value| self.len(value));
    }

    fn labels(&mut self, labels: &[u8]) {
        self.len(labels.len());
        self.0.extend(labels);
    }

    fn op(&mut self, op: &Op) {
        match op {
            Op::Alias => self.byte(0),
            Op::Unary(kind) => {
                self.byte(1);
                self.byte(*kind as u8);
            }
            Op::Binary(kind) => {
                self.byte(2);
                self.byte(*kind as u8);
            }
            Op::SumReduce { axes } => {
                self.byte(3);
                self.list(axes);
            }
            Op::Slice { axis, start, end } => {
                self.byte(4);
                self.list(&[*axis, *start, *end]);
            }
            Op::Concat { axis } => {
                self.byte(5);
                self.len(*axis);
            }
            Op::Transpose { perm } => {
                self.byte(6);
                self.list(perm);
            }
            Op::EinSum { a, b, out } => {
                self.byte(7);
                self.labels(a);
                self.labels(b);
                self.labels(out);
            }
            Op::Conv(spec) => {
                self.byte(8);
                self.byte(u8::from(spec.channels_last));
                self.byte(u8::from(spec.batched));
                self.len(spec.group);
                self.list(&spec.strides);
                self.list(&spec.dilations);
                self.list(&spec.pads_before);
                self.list(&spec.pads_after);
            }
            Op::Gru { hidden, backward } => {
                self.byte(9);
                self.len(*hidden);
                self.byte(u8::from(*backward));
            }
            Op::RmsNorm { axis, eps } => {
                self.byte(10);
                self.len(*axis);
                self.float(*eps);
            }
            Op::Gather { axis, indices } => {
                self.byte(11);
                self.len(*axis);
                self.list(indices);
            }
            Op::PadReflect { before, after } => {
                self.byte(12);
                self.list(before);
                self.list(after);
            }
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], NetError> {
        let end = self
            .at
            .checked_add(count)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| NetError::Format("truncated".into()))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], NetError> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn byte(&mut self) -> Result<u8, NetError> {
        Ok(self.array::<1>()?[0])
    }

    fn flag(&mut self) -> Result<bool, NetError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(NetError::Format(format!("bad flag {other}"))),
        }
    }

    fn len(&mut self) -> Result<usize, NetError> {
        usize::try_from(u64::from_le_bytes(self.array()?))
            .map_err(|_| NetError::Format("length overflows".into()))
    }

    fn count(&mut self, item_bytes: usize) -> Result<usize, NetError> {
        let count = self.len()?;
        if count.saturating_mul(item_bytes) > self.bytes.len() - self.at {
            return Err(NetError::Format("truncated".into()));
        }
        Ok(count)
    }

    fn float(&mut self) -> Result<f32, NetError> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    fn text(&mut self) -> Result<String, NetError> {
        let len = self.count(1)?;
        String::from_utf8(self.take(len)?.to_vec())
            .map_err(|_| NetError::Format("text is not utf-8".into()))
    }

    fn list(&mut self) -> Result<Vec<usize>, NetError> {
        (0..self.count(8)?).map(|_| self.len()).collect()
    }

    fn labels(&mut self) -> Result<Vec<u8>, NetError> {
        let len = self.count(1)?;
        Ok(self.take(len)?.to_vec())
    }

    fn value(&mut self) -> Result<Value, NetError> {
        let shape = self.list()?;
        let data = match self.byte()? {
            0 => None,
            1 => Some(Weights::F32(
                (0..self.count(4)?)
                    .map(|_| self.float())
                    .collect::<Result<_, _>>()?,
            )),
            2 => Some(Weights::F16(
                (0..self.count(2)?)
                    .map(|_| Ok(u16::from_le_bytes(self.array()?)))
                    .collect::<Result<_, NetError>>()?,
            )),
            other => return Err(NetError::Format(format!("bad weight type {other}"))),
        };
        Ok(Value { shape, data })
    }

    fn op(&mut self) -> Result<Op, NetError> {
        Ok(match self.byte()? {
            0 => Op::Alias,
            1 => Op::Unary(self.unary()?),
            2 => Op::Binary(self.binary()?),
            3 => Op::SumReduce { axes: self.list()? },
            4 => match self.list()?.as_slice() {
                &[axis, start, end] => Op::Slice { axis, start, end },
                _ => return Err(NetError::Format("bad slice".into())),
            },
            5 => Op::Concat { axis: self.len()? },
            6 => Op::Transpose { perm: self.list()? },
            7 => Op::EinSum {
                a: self.labels()?,
                b: self.labels()?,
                out: self.labels()?,
            },
            8 => Op::Conv(ConvSpec {
                channels_last: self.flag()?,
                batched: self.flag()?,
                group: self.len()?,
                strides: self.list()?,
                dilations: self.list()?,
                pads_before: self.list()?,
                pads_after: self.list()?,
            }),
            9 => Op::Gru {
                hidden: self.len()?,
                backward: self.flag()?,
            },
            10 => Op::RmsNorm {
                axis: self.len()?,
                eps: self.float()?,
            },
            11 => Op::Gather {
                axis: self.len()?,
                indices: self.list()?,
            },
            12 => Op::PadReflect {
                before: self.list()?,
                after: self.list()?,
            },
            other => return Err(NetError::Format(format!("unknown op {other}"))),
        })
    }

    fn unary(&mut self) -> Result<Unary, NetError> {
        Ok(match self.byte()? {
            0 => Unary::Sqrt,
            1 => Unary::Rsqrt,
            2 => Unary::Square,
            3 => Unary::Ln,
            4 => Unary::Sigmoid,
            5 => Unary::Tanh,
            other => return Err(NetError::Format(format!("unknown unary {other}"))),
        })
    }

    fn binary(&mut self) -> Result<Binary, NetError> {
        Ok(match self.byte()? {
            0 => Binary::Add,
            1 => Binary::Sub,
            2 => Binary::Mul,
            3 => Binary::Max,
            4 => Binary::Min,
            other => return Err(NetError::Format(format!("unknown binary {other}"))),
        })
    }
}
