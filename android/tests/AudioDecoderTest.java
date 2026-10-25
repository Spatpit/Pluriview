package io.pluriview.viewer;

import android.media.AudioTrack;
import android.media.MediaCodec;
import java.io.*;

/** Runs the actual AudioDecoder against isolated deterministic media fixtures. */
public final class AudioDecoderTest {
    private static void check(boolean value, String message) { if (!value) throw new AssertionError(message); }
    private static AudioPacket packet() throws IOException {
        return new AudioPacket(new DataInputStream(new ByteArrayInputStream(new byte[] {0,0,0,1,0})));
    }
    private static void stopped(DecoderSlot slot) throws Exception {
        long deadline = System.nanoTime() + 1_000_000_000L;
        while (!slot.available() && System.nanoTime() < deadline) Thread.sleep(2);
        check(slot.available(), "Decoder did not stop");
        check(AudioTrack.latest.released, "Speaker output leaked");
    }
    public static void main(String[] args) throws Exception {
        DecoderSlot slot = new DecoderSlot();
        AudioTrack.pressureMillis = 80;
        try (AudioDecoder decoder = new AudioDecoder(slot)) {
            decoder.awaitReady();
            // A burst larger than the six-record queue must drain while speaker
            // writes temporarily return zero, instead of repeatedly reconnecting.
            for (int i = 0; i < 24; i++) decoder.queue(packet());
            long deadline = System.nanoTime() + 1_000_000_000L;
            while (AudioTrack.latest.written < 24 * 4096 && System.nanoTime() < deadline) Thread.sleep(2);
            check(AudioTrack.latest.written == 24 * 4096, "Burst audio did not reach speaker output");
            check(AudioTrack.latest.playing, "Speaker never started");
        }
        stopped(slot);
        AudioTrack.pressureMillis = 0;
        AudioTrack.stall = true;
        try (AudioDecoder decoder = new AudioDecoder(slot)) {
            decoder.awaitReady();
            long start = System.nanoTime();
            try {
                for (int i = 0; i < 24; i++) decoder.queue(packet());
                throw new AssertionError("Stuck speaker accepted unbounded audio");
            } catch (IOException expected) {
                check("Audio output stalled".equals(expected.getMessage()), "Speaker failure hidden by queue full: " + expected.getMessage());
                check(System.nanoTime() - start < 500_000_000L, "Speaker stall was unbounded");
            }
        }
        stopped(slot); AudioTrack.stall = false;
        MediaCodec.rejectInput = true;
        try (AudioDecoder decoder = new AudioDecoder(slot)) {
            decoder.awaitReady();
            long start = System.nanoTime();
            try {
                // Late audio is skipped, so a stuck codec never builds a backlog;
                // its watchdog still ends the stream within a bounded time.
                while (System.nanoTime() - start < 3_000_000_000L) decoder.queue(packet());
                throw new AssertionError("Stuck codec accepted unbounded audio");
            } catch (IOException expected) {
                check("Audio decoder stalled".equals(expected.getMessage()), "Wrong codec-stall reason: " + expected.getMessage());
                check(System.nanoTime() - start < 1_500_000_000L, "Codec stall was unbounded");
                check(decoder.dropped() > 0, "Late audio was not skipped");
            }
        }
        stopped(slot); MediaCodec.rejectInput = false;
        // A Wi-Fi stall followed by a burst: audio that is already too late is
        // skipped and playback continues, instead of ending the connection.
        AudioTrack.pressureMillis = 120;
        try (AudioDecoder decoder = new AudioDecoder(slot)) {
            decoder.awaitReady();
            AudioPacket[] burst = new AudioPacket[40];
            for (int i = 0; i < burst.length; i++) burst[i] = packet();
            Thread.sleep(200); // already late when they reach the phone
            for (AudioPacket late : burst) decoder.queue(late);
            long deadline = System.nanoTime() + 1_000_000_000L;
            while (AudioTrack.latest.written == 0 && System.nanoTime() < deadline) Thread.sleep(2);
            check(AudioTrack.latest.written > 0, "Playback did not resume after the burst");
            check(decoder.dropped() > 0, "Stale burst audio was played late");
        }
        AudioTrack.pressureMillis = 0;
        stopped(slot); MediaCodec.rejectInput = false;
        System.out.println("Production audio pump: burst input, partial/zero output, playback start, bounded codec/speaker stalls and cleanup passed");
    }
}
