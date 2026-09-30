use std::hint::black_box;

use criterion::{Criterion, Throughput};
use sdrmm_device::{ByteCoding, ByteConverter, SampleConverter};

const BYTES: usize = 256 * 1024;

fn bytes() -> Vec<u8> {
    (0..BYTES)
        .map(|n| (n as u32).wrapping_mul(2_654_435_761).to_le_bytes()[3])
        .collect()
}

fn conversion(c: &mut Criterion) {
    let input = bytes();
    let mut group = c.benchmark_group("convert");
    group.throughput(Throughput::Bytes(BYTES as u64));
    for (label, coding) in [
        ("cs8", ByteCoding::TwosComplement { full_scale: 128.0 }),
        (
            "cu8",
            ByteCoding::OffsetBinary {
                offset: 127.4,
                full_scale: 127.5,
            },
        ),
    ] {
        let mut converter = ByteConverter::new(coding, BYTES / 2);
        group.bench_function(label, |b| {
            b.iter(|| {
                black_box(converter.convert(black_box(&input)));
            });
        });
    }
    group.finish();
}

criterion::criterion_group!(benches, conversion);
criterion::criterion_main!(benches);
