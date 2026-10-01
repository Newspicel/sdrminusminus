#include <complex.h>
#include <string.h>

#include <liquid/liquid.h>

#include "bench.h"

#define DDC_INPUT_RATE 20e6
#define DDC_OUTPUT_RATE 48e3
#define DDC_OFFSET 187500.0
#define STOPBAND_DB 60.0f

typedef struct {
    unsigned n;
    float complex *x;
    float complex *y;
    float complex *z;
    float *real;
    firfilt_crcf fir;
    firdecim_crcf decim;
    resamp_crcf resamp;
    nco_crcf nco;
    nco_crcf ddc_nco;
    msresamp_crcf ddc_resamp;
    fftplan fft;
    float complex *fft_in;
    float complex *fft_out;
    freqdem fm;
    unsigned long produced;
} liquid_ctx;

static void fir_step(void *raw) {
    liquid_ctx *c = raw;
    firfilt_crcf_execute_block(c->fir, c->x, c->n, c->y);
}

static void decimate_step(void *raw) {
    liquid_ctx *c = raw;
    firdecim_crcf_execute_block(c->decim, c->x, c->n / BENCH_DECIMATION, c->y);
}

static void resample_step(void *raw) {
    liquid_ctx *c = raw;
    unsigned produced = 0;
    resamp_crcf_execute_block(c->resamp, c->x, c->n, c->y, &produced);
    c->produced += produced;
}

static void nco_step(void *raw) {
    liquid_ctx *c = raw;
    nco_crcf_mix_block_up(c->nco, c->x, c->y, c->n);
}

static void ddc_step(void *raw) {
    liquid_ctx *c = raw;
    unsigned produced = 0;
    nco_crcf_mix_block_down(c->ddc_nco, c->x, c->y, c->n);
    msresamp_crcf_execute(c->ddc_resamp, c->y, c->n, c->z, &produced);
    c->produced += produced;
}

static void fft_step(void *raw) {
    liquid_ctx *c = raw;
    memcpy(c->fft_in, c->x, sizeof(float complex) * BENCH_FFT);
    fft_execute(c->fft);
}

static void fm_step(void *raw) {
    liquid_ctx *c = raw;
    freqdem_demodulate_block(c->fm, c->x, c->n, c->real);
}

static int setup(liquid_ctx *c, unsigned n) {
    float taps[BENCH_TAPS];
    bench_taps(taps, BENCH_TAPS, BENCH_CUTOFF);
    c->n = n;
    c->produced = 0;
    c->x = malloc(sizeof(float complex) * n);
    c->y = malloc(sizeof(float complex) * 2 * n);
    c->z = malloc(sizeof(float complex) * 2 * n);
    c->real = malloc(sizeof(float) * n);
    c->fft_in = malloc(sizeof(float complex) * BENCH_FFT);
    c->fft_out = malloc(sizeof(float complex) * BENCH_FFT);
    if (!c->x || !c->y || !c->z || !c->real || !c->fft_in || !c->fft_out || n < BENCH_FFT)
        return 1;
    bench_signal((float *)c->x, n, 0x11D);
    c->fir = firfilt_crcf_create(taps, BENCH_TAPS);
    c->decim = firdecim_crcf_create(BENCH_DECIMATION, taps, BENCH_TAPS);
    c->resamp = resamp_crcf_create_default(48000.0f / 44100.0f);
    c->nco = nco_crcf_create(LIQUID_NCO);
    nco_crcf_set_frequency(c->nco, (float)(2.0 * M_PI * DDC_OFFSET / DDC_INPUT_RATE));
    c->ddc_nco = nco_crcf_create(LIQUID_NCO);
    nco_crcf_set_frequency(c->ddc_nco, (float)(2.0 * M_PI * DDC_OFFSET / DDC_INPUT_RATE));
    c->ddc_resamp = msresamp_crcf_create((float)(DDC_OUTPUT_RATE / DDC_INPUT_RATE), STOPBAND_DB);
    c->fft = fft_create_plan(BENCH_FFT, c->fft_in, c->fft_out, LIQUID_FFT_FORWARD, 0);
    c->fm = freqdem_create(0.5f);
    return !c->fir || !c->decim || !c->resamp || !c->nco || !c->ddc_nco || !c->ddc_resamp ||
           !c->fft || !c->fm;
}

int main(int argc, char **argv) {
    bench_timing timing;
    if (bench_parse(argc, argv, &timing)) return 1;
    liquid_ctx c;
    if (setup(&c, timing.block)) {
        fprintf(stderr, "liquid-dsp setup failed\n");
        return 1;
    }
    printf("version\t%s\n", liquid_libversion());
    bench_run("fir", fir_step, &c, c.n, &timing);
    bench_run("decimate", decimate_step, &c, c.n, &timing);
    bench_run("resample", resample_step, &c, c.n, &timing);
    bench_ratio("resample", resample_step, &c, c.n, &c.produced);
    bench_run("nco", nco_step, &c, c.n, &timing);
    bench_run("ddc", ddc_step, &c, c.n, &timing);
    bench_ratio("ddc", ddc_step, &c, c.n, &c.produced);
    bench_run("fft", fft_step, &c, BENCH_FFT, &timing);
    bench_run("fm", fm_step, &c, c.n, &timing);
    return 0;
}
