#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "aacenc_lib.h"
#include "aacdecoder_lib.h"

static void check(int code, const char *operation) {
    if (code != 0) { fprintf(stderr, "%s: 0x%x\n", operation, code); exit(1); }
}

static void write_pcm(FILE *pcm, HANDLE_AACDECODER decoder, INT_PCM *samples) {
    CStreamInfo *info = aacDecoder_GetStreamInfo(decoder);
    for (int i = 0; i < info->frameSize; ++i) {
        float pair[2] = { samples[i * info->numChannels] / 32768.0f,
            samples[i * info->numChannels + (info->numChannels == 1 ? 0 : 1)] / 32768.0f };
        fwrite(pair, sizeof(float), 2, pcm);
    }
}

static int drm(const char *base, const char *sdc) {
    char path[1024];
    snprintf(path, sizeof(path), "%s.drm", base); FILE *units = fopen(path, "rb");
    snprintf(path, sizeof(path), "%s.drm.pcm", base); FILE *pcm = fopen(path, "wb");
    if (!units || !pcm) return 3;
    unsigned value = (unsigned)strtoul(sdc, NULL, 16);
    UCHAR config[2] = { value >> 8, value & 255 };
    UCHAR *pointer = config; UINT size = 2;
    HANDLE_AACDECODER decoder = aacDecoder_Open(TT_DRM, 1);
    check(aacDecoder_ConfigRaw(decoder, &pointer, &size), "DRM config");
    UCHAR bytes[8192]; unsigned char length[2]; INT_PCM samples[16384];
    while (fread(length, 1, 2, units) == 2) {
        size = length[0] * 256 + length[1];
        if (size > sizeof(bytes) || fread(bytes, 1, size, units) != size) return 4;
        pointer = bytes; UINT valid = size;
        check(aacDecoder_Fill(decoder, &pointer, &size, &valid), "fill decoder");
        check(aacDecoder_DecodeFrame(decoder, samples, 16384, 0), "decode DRM frame");
        write_pcm(pcm, decoder, samples);
    }
    fclose(units); fclose(pcm); aacDecoder_Close(decoder);
    return 0;
}

static int encode(const char *base, int aot, int rate, int channels, int bitrate) {
    HANDLE_AACENCODER encoder = NULL;
    check(aacEncOpen(&encoder, 0, channels), "open encoder");
    check(aacEncoder_SetParam(encoder, AACENC_AOT, aot), "object type");
    check(aacEncoder_SetParam(encoder, AACENC_SAMPLERATE, rate), "rate");
    check(aacEncoder_SetParam(encoder, AACENC_CHANNELMODE, channels == 1 ? MODE_1 : MODE_2), "channels");
    check(aacEncoder_SetParam(encoder, AACENC_BITRATE, bitrate), "bitrate");
    check(aacEncoder_SetParam(encoder, AACENC_GRANULE_LENGTH, 960), "frame length");
    check(aacEncoder_SetParam(encoder, AACENC_TRANSMUX, TT_MP4_RAW), "transport");
    check(aacEncEncode(encoder, NULL, NULL, NULL, NULL), "initialize");
    AACENC_InfoStruct info = {0};
    check(aacEncInfo(encoder, &info), "information");
    char path[1024];
    snprintf(path, sizeof(path), "%s.aus", base); FILE *units = fopen(path, "wb");
    snprintf(path, sizeof(path), "%s.pcm", base); FILE *pcm = fopen(path, "wb");
    snprintf(path, sizeof(path), "%s.asc", base); FILE *asc = fopen(path, "wb");
    if (!units || !pcm || !asc) return 3;
    fwrite(info.confBuf, 1, info.confSize, asc); fclose(asc);
    HANDLE_AACDECODER decoder = aacDecoder_Open(TT_MP4_RAW, 1);
    UCHAR *config = info.confBuf; UINT config_size = info.confSize;
    check(aacDecoder_ConfigRaw(decoder, &config, &config_size), "decoder config");
    INT_PCM input[8192], reference[16384]; UCHAR output[8192];
    for (int frame = 0; frame < 30; ++frame) {
        int samples = info.frameLength * channels;
        for (int i = 0; i < samples; ++i) {
            int channel = i % channels;
            double t = (frame * info.frameLength + i / channels) / (double)rate;
            input[i] = (INT_PCM)(7000 * sin(6.283185307179586 * (channel == 0 ? 700 : 1300) * t));
        }
        void *in_pointer = input, *out_pointer = output;
        INT in_id = IN_AUDIO_DATA, out_id = OUT_BITSTREAM_DATA;
        INT in_size = samples * sizeof(INT_PCM), out_size = sizeof(output);
        INT in_element = sizeof(INT_PCM), out_element = 1;
        AACENC_BufDesc in = {1, &in_pointer, &in_id, &in_size, &in_element};
        AACENC_BufDesc out = {1, &out_pointer, &out_id, &out_size, &out_element};
        AACENC_InArgs args = {0}; AACENC_OutArgs result = {0}; args.numInSamples = samples;
        check(aacEncEncode(encoder, &in, &out, &args, &result), "encode");
        if (result.numOutBytes == 0) continue;
        unsigned char length[2] = { result.numOutBytes >> 8, result.numOutBytes & 255 };
        fwrite(length, 1, 2, units); fwrite(output, 1, result.numOutBytes, units);
        UCHAR *packet = output; UINT size = result.numOutBytes, valid = size;
        check(aacDecoder_Fill(decoder, &packet, &size, &valid), "fill decoder");
        check(aacDecoder_DecodeFrame(decoder, reference, 16384, 0), "decode reference");
        write_pcm(pcm, decoder, reference);
    }
    fclose(units); fclose(pcm); aacDecoder_Close(decoder); aacEncClose(&encoder);
    return 0;
}

int main(int argc, char **argv) {
    if (argc == 4 && strcmp(argv[1], "drm") == 0) return drm(argv[2], argv[3]);
    if (argc == 6) return encode(argv[1], atoi(argv[2]), atoi(argv[3]), atoi(argv[4]), atoi(argv[5]));
    return 2;
}
