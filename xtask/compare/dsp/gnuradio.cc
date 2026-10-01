#include <gnuradio/analog/quadrature_demod_cf.h>
#include <gnuradio/blocks/rotator.h>
#include <gnuradio/constants.h>
#include <gnuradio/fft/fft.h>
#include <gnuradio/filter/fir_filter.h>
#include <gnuradio/filter/firdes.h>
#include <gnuradio/filter/freq_xlating_fir_filter.h>
#include <gnuradio/filter/pfb_arb_resampler.h>
#include <gnuradio/logger.h>
#include <spdlog/sinks/dist_sink.h>
#include <spdlog/sinks/stdout_sinks.h>
#include <volk/volk_version.h>

#include <algorithm>
#include <complex>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

#include "bench.h"

namespace {

constexpr double ddc_input_rate = 20e6;
constexpr double ddc_output_rate = 48e3;
constexpr double ddc_offset = 187500.0;
constexpr int ddc_decimation = 25;
constexpr double ddc_passband = 0.4 * ddc_output_rate;
constexpr double ddc_protect = 0.5 * ddc_output_rate;
constexpr double stopband_db = 60.0;
constexpr unsigned arb_filters = 32;

using complex = gr_complex;
using samples = std::vector<complex>;

struct history {
    samples buffer;
    std::size_t keep;

    explicit history(std::size_t keep) : buffer(keep, complex{}), keep(keep) {}

    void append(const complex* data, std::size_t n) { buffer.insert(buffer.end(), data, data + n); }

    void consume(std::size_t n) { buffer.erase(buffer.begin(), buffer.begin() + n); }
};

struct arb_stage {
    gr::filter::kernel::pfb_arb_resampler_ccf kernel;
    history input;
    samples output;
    unsigned long produced = 0;

    arb_stage(float rate, const std::vector<float>& taps)
        : kernel(rate, taps, arb_filters), input(kernel.taps_per_filter() - 1)
    {
    }

    void process(const complex* data, std::size_t n)
    {
        input.append(data, n);
        int available = int(input.buffer.size()) - int(kernel.taps_per_filter()) + 1;
        if (available <= 0) return;
        output.resize(std::size_t(available * kernel.fractional_rate() * kernel.interpolation_rate()) + 64);
        int consumed = 0;
        int made = kernel.filter(output.data(), input.buffer.data(), available, consumed);
        output.resize(std::size_t(made));
        produced += std::size_t(made);
        input.consume(std::size_t(consumed));
    }
};

struct xlating_stage {
    gr::filter::freq_xlating_fir_filter_ccf::sptr block;
    history input;
    samples output;
    unsigned ntaps;

    explicit xlating_stage(const std::vector<float>& taps)
        : block(gr::filter::freq_xlating_fir_filter_ccf::make(
              ddc_decimation, taps, ddc_offset, ddc_input_rate)),
          input(taps.size() - 1),
          ntaps(unsigned(taps.size()))
    {
    }

    void process(const complex* data, std::size_t n)
    {
        input.append(data, n);
        std::size_t usable = input.buffer.size() - (ntaps - 1);
        int produced = int(usable / ddc_decimation);
        output.resize(std::size_t(produced));
        gr_vector_const_void_star in{ input.buffer.data() };
        gr_vector_void_star out{ output.data() };
        int done = block->work(produced, in, out);
        output.resize(std::size_t(done));
        input.consume(std::size_t(done) * ddc_decimation);
    }
};

struct context {
    unsigned n;
    samples x;
    samples padded;
    samples y;
    std::vector<float> real;
    std::unique_ptr<gr::filter::kernel::fir_filter_ccf> fir;
    std::unique_ptr<arb_stage> resample;
    gr::blocks::rotator rotator;
    std::unique_ptr<xlating_stage> xlate;
    std::unique_ptr<arb_stage> ddc_resample;
    std::unique_ptr<gr::fft::fft_complex_fwd> fft;
    gr::analog::quadrature_demod_cf::sptr fm;
};

void fir_step(void* raw)
{
    auto* c = static_cast<context*>(raw);
    c->fir->filterN(c->y.data(), c->padded.data(), c->n);
}

void decimate_step(void* raw)
{
    auto* c = static_cast<context*>(raw);
    c->fir->filterNdec(c->y.data(), c->padded.data(), c->n / BENCH_DECIMATION, BENCH_DECIMATION);
}

void resample_step(void* raw)
{
    auto* c = static_cast<context*>(raw);
    c->resample->process(c->x.data(), c->n);
}

void nco_step(void* raw)
{
    auto* c = static_cast<context*>(raw);
    c->rotator.rotateN(c->y.data(), c->x.data(), int(c->n));
}

void ddc_step(void* raw)
{
    auto* c = static_cast<context*>(raw);
    c->xlate->process(c->x.data(), c->n);
    c->ddc_resample->process(c->xlate->output.data(), c->xlate->output.size());
}

void fft_step(void* raw)
{
    auto* c = static_cast<context*>(raw);
    std::memcpy(c->fft->get_inbuf(), c->x.data(), sizeof(complex) * BENCH_FFT);
    c->fft->execute();
}

void fm_step(void* raw)
{
    auto* c = static_cast<context*>(raw);
    gr_vector_const_void_star in{ c->padded.data() };
    gr_vector_void_star out{ c->real.data() };
    c->fm->work(int(c->n), in, out);
}

std::vector<float> fir_taps()
{
    std::vector<float> taps(BENCH_TAPS);
    bench_taps(taps.data(), BENCH_TAPS, BENCH_CUTOFF);
    return taps;
}

std::vector<float> arb_taps(double rate)
{
    double cutoff = 0.4 * std::min(1.0, rate);
    return gr::filter::firdes::low_pass_2(
        arb_filters, arb_filters, cutoff, 0.2 * std::min(1.0, rate), 100.0);
}

std::vector<float> xlating_taps()
{
    double stage_rate = ddc_input_rate / ddc_decimation;
    return gr::filter::firdes::low_pass_2(
        1.0, ddc_input_rate, ddc_passband, stage_rate - ddc_protect - ddc_passband, stopband_db);
}

std::vector<float> ddc_arb_taps()
{
    double stage_rate = ddc_input_rate / ddc_decimation;
    return gr::filter::firdes::low_pass_2(arb_filters,
                                          arb_filters * stage_rate,
                                          ddc_passband,
                                          ddc_output_rate - ddc_protect - ddc_passband,
                                          stopband_db);
}

void setup(context& c, unsigned n)
{
    c.n = n;
    c.x.resize(n);
    bench_signal(reinterpret_cast<float*>(c.x.data()), n, 0x11D);
    c.padded = samples(BENCH_TAPS - 1, complex{});
    c.padded.insert(c.padded.end(), c.x.begin(), c.x.end());
    c.y.resize(2 * n);
    c.real.resize(n);
    c.fir = std::make_unique<gr::filter::kernel::fir_filter_ccf>(fir_taps());
    double resample_rate = 48000.0 / 44100.0;
    c.resample = std::make_unique<arb_stage>(float(resample_rate), arb_taps(resample_rate));
    c.rotator.set_phase_incr(std::polar(1.0f, float(2.0 * M_PI * ddc_offset / ddc_input_rate)));
    c.xlate = std::make_unique<xlating_stage>(xlating_taps());
    c.ddc_resample = std::make_unique<arb_stage>(
        float(ddc_output_rate * ddc_decimation / ddc_input_rate), ddc_arb_taps());
    c.fft = std::make_unique<gr::fft::fft_complex_fwd>(BENCH_FFT, 1);
    c.fm = gr::analog::quadrature_demod_cf::make(0.5f);
}

}

void log_to_stderr()
{
    auto backend = std::static_pointer_cast<spdlog::sinks::dist_sink_mt>(
        gr::logging::singleton().default_backend());
    backend->set_sinks({ std::make_shared<spdlog::sinks::stderr_sink_mt>() });
}

int main(int argc, char** argv)
{
    log_to_stderr();
    bench_timing timing;
    if (bench_parse(argc, argv, &timing)) return 1;
    if (timing.block < BENCH_FFT) {
        std::fprintf(stderr, "block must hold one FFT\n");
        return 1;
    }
    context c;
    setup(c, timing.block);
    std::printf("version\t%s (VOLK %d.%d.%d)\n",
                gr::version().c_str(),
                VOLK_VERSION_MAJOR,
                VOLK_VERSION_MINOR,
                VOLK_VERSION_MAINT);
    bench_run("fir", fir_step, &c, c.n, &timing);
    bench_run("decimate", decimate_step, &c, c.n, &timing);
    bench_run("resample", resample_step, &c, c.n, &timing);
    bench_ratio("resample", resample_step, &c, c.n, &c.resample->produced);
    bench_run("nco", nco_step, &c, c.n, &timing);
    bench_run("ddc", ddc_step, &c, c.n, &timing);
    bench_ratio("ddc", ddc_step, &c, c.n, &c.ddc_resample->produced);
    bench_run("fft", fft_step, &c, BENCH_FFT, &timing);
    bench_run("fm", fm_step, &c, c.n, &timing);
    return 0;
}
