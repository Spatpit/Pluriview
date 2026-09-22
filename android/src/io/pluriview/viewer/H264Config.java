package io.pluriview.viewer;

import java.io.ByteArrayOutputStream;
import java.io.IOException;

/** Extract initial SPS/PPS for MediaCodec's explicit codec-configuration API. */
final class H264Config {
    final byte[] sps, pps, frame;

    H264Config(byte[] accessUnit) throws IOException {
        byte[] sequence = null, picture = null;
        ByteArrayOutputStream slices = new ByteArrayOutputStream();
        boolean idr = false;
        int start = findStart(accessUnit, 0);
        if (start != 0) throw new IOException("Expected Annex-B video");
        while (start >= 0) {
            int payload = start + (accessUnit[start + 2] == 1 ? 3 : 4);
            int next = findStart(accessUnit, payload);
            int end = next < 0 ? accessUnit.length : next;
            if (payload >= end) throw new IOException("Empty video NAL");
            int type = accessUnit[payload] & 31;
            if (type == 7 || type == 8) {
                byte[] config = new byte[4 + end - payload]; config[3] = 1;
                System.arraycopy(accessUnit, payload, config, 4, end - payload);
                if (type == 7) sequence = config; else picture = config;
            } else {
                slices.write(accessUnit, start, end - start);
                if (type == 5) idr = true;
            }
            start = next;
        }
        if (sequence == null || picture == null || !idr) throw new IOException("Missing initial keyframe configuration");
        sps = sequence; pps = picture; frame = slices.toByteArray();
    }

    private static int findStart(byte[] bytes, int from) {
        for (int i = from; i + 2 < bytes.length; i++) {
            if (bytes[i] == 0 && bytes[i + 1] == 0
                && (bytes[i + 2] == 1 || (i + 3 < bytes.length && bytes[i + 2] == 0 && bytes[i + 3] == 1))) return i;
        }
        return -1;
    }
}
