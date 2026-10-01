#ifndef SDRMM_COMPARE_BENCH_H
#define SDRMM_COMPARE_BENCH_H

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#define BENCH_TAPS 127
#define BENCH_CUTOFF 0.11
#define BENCH_DECIMATION 4
#define BENCH_FFT 4096
#define BENCH_MAX_REPS 1001

typedef struct {
    unsigned block;
    unsigned reps;
    double rep_seconds;
} bench_timing;

typedef void (*bench_step)(void *ctx);

static inline double bench_now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec + (double)t.tv_nsec * 1e-9;
}

static inline int bench_order(const void *a, const void *b) {
    double x = *(const double *)a;
    double y = *(const double *)b;
    return (x > y) - (x < y);
}

static inline int bench_parse(int argc, char **argv, bench_timing *timing) {
    if (argc != 4) {
        fprintf(stderr, "usage: %s <block> <reps> <rep_seconds>\n", argv[0]);
        return 1;
    }
    timing->block = (unsigned)strtoul(argv[1], NULL, 10);
    timing->reps = (unsigned)strtoul(argv[2], NULL, 10);
    timing->rep_seconds = strtod(argv[3], NULL);
    if (timing->block == 0 || timing->reps == 0 || timing->reps > BENCH_MAX_REPS ||
        !(timing->rep_seconds > 0.0)) {
        fprintf(stderr, "invalid timing arguments\n");
        return 1;
    }
    return 0;
}

static inline unsigned long bench_calibrate(bench_step step, void *ctx, double seconds) {
    unsigned long iterations = 1;
    for (;;) {
        double start = bench_now();
        for (unsigned long i = 0; i < iterations; i++) step(ctx);
        if (bench_now() - start >= seconds) return iterations;
        iterations *= 2;
    }
}

static inline void bench_run(const char *id, bench_step step, void *ctx, double samples,
                      const bench_timing *timing) {
    static double times[BENCH_MAX_REPS];
    unsigned long iterations = bench_calibrate(step, ctx, timing->rep_seconds);
    for (unsigned r = 0; r < timing->reps; r++) {
        double start = bench_now();
        for (unsigned long i = 0; i < iterations; i++) step(ctx);
        times[r] = bench_now() - start;
    }
    qsort(times, timing->reps, sizeof(double), bench_order);
    double median = times[timing->reps / 2];
    printf("%s\t%.6f\n", id, samples * (double)iterations / median / 1e6);
    fflush(stdout);
}

static inline void bench_ratio(const char *id, bench_step step, void *ctx, double samples,
                               const unsigned long *produced) {
    const unsigned steps = 64;
    unsigned long before = *produced;
    for (unsigned i = 0; i < steps; i++) step(ctx);
    printf("ratio\t%s\t%.9f\n", id, (double)(*produced - before) / (samples * steps));
    fflush(stdout);
}

static inline float bench_random(unsigned *state) {
    *state = *state * 1664525u + 1013904223u;
    return (float)(*state >> 8) / 8388608.0f - 1.0f;
}

static inline void bench_signal(float *interleaved, unsigned n, unsigned seed) {
    unsigned state = seed;
    for (unsigned i = 0; i < 2 * n; i++) interleaved[i] = 0.7f * bench_random(&state);
}

static inline void bench_taps(float *taps, unsigned n, double cutoff) {
    double middle = (n - 1) / 2.0;
    for (unsigned k = 0; k < n; k++) {
        double x = k - middle;
        double sinc = x == 0.0 ? 2.0 * cutoff : sin(2.0 * M_PI * cutoff * x) / (M_PI * x);
        double window = 0.5 - 0.5 * cos(2.0 * M_PI * k / (n - 1));
        taps[k] = (float)(sinc * window);
    }
}

#endif
