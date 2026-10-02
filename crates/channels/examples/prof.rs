use std::sync::Arc;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let net = Arc::new(sdrmm_channels::neural::Net::load(&std::fs::read(path).unwrap()).unwrap());
    let mut s = sdrmm_channels::neural::Session::new(net.clone());
    let n = net.input_len(0).unwrap();
    for i in 0..n { s.input_mut(0)[i] = ((i * 7919) % 13) as f32 * 0.01; }
    let t = std::time::Instant::now();
    for _ in 0..300 { s.run(); }
    eprintln!("{:.0} us/frame", t.elapsed().as_secs_f64() / 300.0 * 1e6);
    sdrmm_channels::neural::dump_profile();
}
