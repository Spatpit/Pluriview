package io.pluriview.viewer;

import android.media.AudioAttributes;
import android.media.AudioFormat;
import android.media.AudioTrack;
import android.media.MediaCodec;
import android.media.MediaFormat;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.TimeUnit;

/** AAC decoding and bounded speaker writes never block the video/UI workers. */
final class AudioDecoder implements AutoCloseable {
    private final ArrayBlockingQueue<AudioPacket> packets = new ArrayBlockingQueue<>(6);
    private final AudioStartup startup = new AudioStartup();
    private final Thread worker;
    private volatile boolean closed;
    private volatile IOException failure;
    private volatile AudioTrack track;
    AudioDecoder(DecoderSlot slot) throws IOException {
        worker = new Thread(this::decode, "viewer-audio-decoder"); slot.start(worker);
    }
    void awaitReady() throws IOException { startup.await(); if (failure != null) throw failure; }
    void queue(AudioPacket packet) throws IOException {
        // TCP may deliver a short burst after Wi-Fi scheduling. Let the decoder
        // consume the same bounded queue instead of reconnecting on one full
        // offer. Packet age still bounds the total wait, including network time.
        while (!closed) {
            if (failure != null) throw failure;
            long remaining = 250_000_000L - (System.nanoTime() - packet.received);
            if (remaining <= 0) throw new IOException("Audio input queue stalled");
            try {
                if (packets.offer(packet, Math.min(remaining, 10_000_000L), TimeUnit.NANOSECONDS)) return;
            } catch (InterruptedException stopped) {
                Thread.currentThread().interrupt();
                throw new java.io.InterruptedIOException("Audio stopped");
            }
        }
        throw new IOException("Audio stopped");
    }
    private void decode() {
        MediaCodec codec = null;
        AudioTrack output = null;
        String stage = "Audio decoder unavailable";
        try {
            MediaFormat format = MediaFormat.createAudioFormat("audio/mp4a-latm", 48000, 2);
            format.setByteBuffer("csd-0", ByteBuffer.wrap(new byte[] {0x11, (byte) 0x90}));
            format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 8192);
            codec = MediaCodec.createDecoderByType("audio/mp4a-latm");
            codec.configure(format, null, null, 0); codec.start();
            stage = "Audio output unavailable";
            int minimum = AudioTrack.getMinBufferSize(48000, AudioFormat.CHANNEL_OUT_STEREO, AudioFormat.ENCODING_PCM_16BIT);
            if (minimum < 0 || minimum > 38400) throw new IOException("Audio output unavailable");
            output = new AudioTrack.Builder()
                .setAudioAttributes(new AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build())
                .setAudioFormat(new AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT).setSampleRate(48000).setChannelMask(AudioFormat.CHANNEL_OUT_STEREO).build())
                .setBufferSizeInBytes(Math.max(minimum, 8192)).setTransferMode(AudioTrack.MODE_STREAM).build();
            track = output;
            if (closed) return;
            MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
            long frame = 0, outputDeadline = 0;
            int outputIndex = -1;
            ByteBuffer pcm = null;
            // Streaming AudioTrack owns its device-specific start threshold.
            // Waiting for a hard-coded byte count before play can deadlock a
            // smaller effective output buffer, filling the encoded input queue.
            output.play();
            DecoderWatchdog watchdog = new DecoderWatchdog();
            startup.finish(null);
            stage = "Audio playback failed";
            while (!closed) {
                boolean worked = false;
                AudioPacket packet = packets.peek();
                if (packet != null) {
                    if (System.nanoTime() - packet.received > 250_000_000L) throw new IOException("Audio input queue stalled");
                    int index = codec.dequeueInputBuffer(0);
                    if (index >= 0) {
                        packets.remove();
                        ByteBuffer input = codec.getInputBuffer(index);
                        if (input == null || input.capacity() < packet.bytes.length) throw new IOException("Audio packet too large");
                        input.clear(); input.put(packet.bytes);
                        codec.queueInputBuffer(index, 0, packet.bytes.length, frame * 1024_000_000L / 48000, 0);
                        frame++; worked = true;
                    }
                }
                int index = outputIndex < 0 ? codec.dequeueOutputBuffer(info, 0) : MediaCodec.INFO_TRY_AGAIN_LATER;
                if (index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                    MediaFormat actual = codec.getOutputFormat();
                    if (actual.getInteger(MediaFormat.KEY_SAMPLE_RATE) != 48000 || actual.getInteger(MediaFormat.KEY_CHANNEL_COUNT) != 2
                        || (actual.containsKey(MediaFormat.KEY_PCM_ENCODING) && actual.getInteger(MediaFormat.KEY_PCM_ENCODING) != AudioFormat.ENCODING_PCM_16BIT))
                        throw new IOException("Unsupported decoded audio");
                    worked = true;
                } else if (index >= 0) {
                    outputIndex = index;
                    pcm = codec.getOutputBuffer(index);
                    if (pcm == null || info.size < 0 || info.size > 16384 || info.offset < 0 || info.offset > pcm.capacity() - info.size)
                        throw new IOException("Invalid decoded audio");
                    pcm.limit(info.offset + info.size); pcm.position(info.offset);
                    outputDeadline = System.nanoTime() + 150_000_000L;
                    worked = true;
                }
                if (outputIndex >= 0 && !closed) {
                    if (pcm.hasRemaining()) {
                        int n = output.write(pcm, pcm.remaining(), AudioTrack.WRITE_NON_BLOCKING);
                        if (n < 0 || System.nanoTime() >= outputDeadline) throw new IOException("Audio output stalled");
                        worked |= n > 0;
                    }
                    if (!pcm.hasRemaining()) {
                        codec.releaseOutputBuffer(outputIndex, false);
                        outputIndex = -1; pcm = null; worked = true;
                    }
                    // A short/zero speaker write must return to input pumping.
                    // Retain only this one codec-owned output until it is drained.
                }
                if (watchdog.stalled(System.nanoTime(), !packets.isEmpty(), worked)) throw new IOException("Audio decoder stalled");
                if (!worked) Thread.sleep(2);
            }
        } catch (Exception error) { failure = error instanceof IOException ? (IOException) error : new IOException(stage); }
        finally {
            startup.finish(failure != null ? failure : new IOException("Audio stopped"));
            if (output != null) {
                try { output.pause(); output.flush(); } catch (RuntimeException ignored) {}
                try { output.release(); } catch (RuntimeException ignored) {}
            }
            track = null;
            if (codec != null) { try { codec.stop(); } catch (RuntimeException ignored) {} try { codec.release(); } catch (RuntimeException ignored) {} }
        }
    }
    @Override public void close() {
        closed = true; packets.clear(); worker.interrupt();
        startup.finish(new IOException("Audio stopped"));
        // A stale decoder must become inaudible immediately, even if the native
        // codec takes longer to release. Its slot remains held until exit.
        AudioTrack active = track;
        if (active != null) { try { active.setVolume(0); active.pause(); active.flush(); } catch (RuntimeException ignored) {} }
    }
}
