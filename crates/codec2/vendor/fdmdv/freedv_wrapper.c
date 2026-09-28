#include <stdint.h>
#include <stdlib.h>

#include "codec2_fdmdv.h"
#include "fdmdv_internal.h"

struct FDMDV *fdmdv_1600_create(void) { return fdmdv_create(16); }

void fdmdv_1600_destroy(struct FDMDV *modem) { fdmdv_destroy(modem); }

int fdmdv_1600_demod(struct FDMDV *modem, const COMP *input, int nin,
                     uint8_t output[32], int *sync, int *reliable_sync) {
  int decoded[32];
  *reliable_sync = 0;
  fdmdv_demod(modem, decoded, reliable_sync, (COMP *)input, &nin);
  for (int i = 0; i < 32; i++) output[i] = decoded[i] != 0;
  *sync = modem->sync;
  return nin;
}
