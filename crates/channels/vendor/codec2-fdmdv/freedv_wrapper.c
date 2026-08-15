#include <stdint.h>
#include <stdlib.h>

#include "codec2_fdmdv.h"
#include "fdmdv_internal.h"

struct sdrmm_fdmdv_result {
  int next_nin;
  int reliable_sync;
  int sync;
};

struct FDMDV *sdrmm_fdmdv_create(void) { return fdmdv_create(16); }

void sdrmm_fdmdv_destroy(struct FDMDV *modem) { fdmdv_destroy(modem); }

struct sdrmm_fdmdv_result sdrmm_fdmdv_demod(struct FDMDV *modem,
                                             const COMP *input, int nin,
                                             uint8_t output[32]) {
  int decoded[32];
  int reliable_sync = 0;
  fdmdv_demod(modem, decoded, &reliable_sync, (COMP *)input, &nin);
  for (int i = 0; i < 32; i++) output[i] = decoded[i] != 0;
  struct sdrmm_fdmdv_result result = {
      .next_nin = nin,
      .reliable_sync = reliable_sync,
      .sync = modem->sync,
  };
  return result;
}
