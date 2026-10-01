#include <fftw3.h>
#include <string.h>

#include "bench.h"

typedef struct {
    fftwf_complex *input;
    fftwf_complex *work;
    fftwf_complex *output;
    fftwf_plan plan;
} fft_ctx;

static void fft_step(void *raw) {
    fft_ctx *ctx = raw;
    memcpy(ctx->work, ctx->input, sizeof(fftwf_complex) * BENCH_FFT);
    fftwf_execute(ctx->plan);
}

int main(int argc, char **argv) {
    bench_timing timing;
    if (bench_parse(argc, argv, &timing)) return 1;
    const char *version = fftwf_version;
    if (strncmp(version, "fftw-", 5) == 0) version += 5;
    printf("version\t%s\n", version);
    fft_ctx ctx;
    ctx.input = fftwf_malloc(sizeof(fftwf_complex) * BENCH_FFT);
    ctx.work = fftwf_malloc(sizeof(fftwf_complex) * BENCH_FFT);
    ctx.output = fftwf_malloc(sizeof(fftwf_complex) * BENCH_FFT);
    if (!ctx.input || !ctx.work || !ctx.output) return 1;
    ctx.plan = fftwf_plan_dft_1d(BENCH_FFT, ctx.work, ctx.output, FFTW_FORWARD, FFTW_MEASURE);
    bench_signal((float *)ctx.input, BENCH_FFT, 0xFF7);
    bench_run("fft", fft_step, &ctx, BENCH_FFT, &timing);
    fftwf_destroy_plan(ctx.plan);
    fftwf_free(ctx.input);
    fftwf_free(ctx.work);
    fftwf_free(ctx.output);
    return 0;
}
