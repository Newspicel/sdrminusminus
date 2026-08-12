/*
 * Safe-Rust-facing wrapper around mbelib's first-generation AMBE decoder.
 * The bundled mbelib sources retain their ISC copyright notice in COPYRIGHT.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "mbelib.h"

typedef struct {
  mbe_parms current;
  mbe_parms previous;
  mbe_parms enhanced;
} dstar_vocoder;

/* D-STAR's 72 serial bits -> AMBE 3600x2400 code-vector positions. */
static const uint8_t DSTAR_W[72] = {
  0,0, 3,2, 1,1, 0,0, 1,1, 0,0, 3,2, 1,1, 3,2, 1,1, 0,0, 3,2,
  0,0, 3,2, 1,1, 0,0, 1,1, 0,0, 3,2, 1,1, 3,2, 1,1, 0,0, 3,2,
  0,0, 3,2, 1,1, 0,0, 1,1, 0,0, 3,2, 1,1, 3,3, 2,1, 0,0, 3,3
};

static const uint8_t DSTAR_X[72] = {
  10,22, 11,9, 10,22, 11,23, 8,20, 9,21, 10,8, 9,21, 8,6, 7,19, 8,20, 9,7,
  6,18, 7,5, 6,18, 7,19, 4,16, 5,17, 6,4, 5,17, 4,2, 3,15, 4,16, 5,3,
  2,14, 3,1, 2,14, 3,15, 0,12, 1,13, 2,0, 1,13, 0,12, 10,11, 0,12, 1,13
};

dstar_vocoder *sdrmm_dstar_vocoder_new(void) {
  dstar_vocoder *decoder = (dstar_vocoder *)calloc(1, sizeof(dstar_vocoder));
  if (decoder != NULL) {
    mbe_initMbeParms(&decoder->current, &decoder->previous, &decoder->enhanced);
  }
  return decoder;
}

void sdrmm_dstar_vocoder_free(dstar_vocoder *decoder) {
  free(decoder);
}

void sdrmm_dstar_vocoder_reset(dstar_vocoder *decoder) {
  if (decoder != NULL) {
    mbe_initMbeParms(&decoder->current, &decoder->previous, &decoder->enhanced);
  }
}

int sdrmm_dstar_vocoder_decode(dstar_vocoder *decoder, const uint8_t bits[72], float pcm[160]) {
  char frame[4][24];
  char data[49];
  char errors_text[64];
  int errors = 0;
  int errors_total = 0;

  if (decoder == NULL || bits == NULL || pcm == NULL) {
    return -1;
  }
  memset(frame, 0, sizeof(frame));
  memset(data, 0, sizeof(data));
  memset(errors_text, 0, sizeof(errors_text));
  for (size_t i = 0; i < 72; ++i) {
    frame[DSTAR_W[i]][DSTAR_X[i]] = (char)(bits[i] != 0);
  }
  mbe_processAmbe3600x2400Framef(
      pcm, &errors, &errors_total, errors_text, frame, data,
      &decoder->current, &decoder->previous, &decoder->enhanced, 3);
  return errors_total;
}
