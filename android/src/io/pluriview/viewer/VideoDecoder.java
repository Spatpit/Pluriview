package io.pluriview.viewer;

import android.media.MediaCodec;
import android.media.MediaFormat;
import android.os.Build;
import android.view.Surface;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.util.ArrayDeque;
import java.util.HashMap;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.TimeUnit;

/** Decode and presentation run independently, with bounded network backpressure. */
final class VideoDecoder implements AutoCloseable {
    interface Listener { void output(VideoPacket packet); }
    static final class Unavailable extends IOException { Unavailable() { super("Video codec unavailable"); } }
    private final ArrayBlockingQueue<VideoPacket> packets = new ArrayBlockingQueue<>(6);
    private final Thread worker;
    private volatile boolean closed;
    private volatile IOException failure;
    private volatile boolean initialized;
    final int width, height;

    VideoDecoder(Surface surface, VideoPacket first, Listener listener, DecoderSlot slot) throws IOException {
        width = first.width; height = first.height;
        H264Config config = new H264Config(first.bytes);
        worker = new Thread(() -> decode(surface, config, listener), "viewer-decoder");
        slot.start(worker);
    }

    void queue(VideoPacket packet) throws IOException {
        if (failure != null) throw failure;
        try {
            if (closed || !packets.offer(packet, initialized ? 250 : 750, TimeUnit.MILLISECONDS)) throw new IOException("Video decoder backlog");
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt(); throw new IOException("Viewer stopped", interrupted);
        }
        if (failure != null) throw failure;
    }

    private static final class Picture {
        final int index;
        final long due;
        final VideoPacket packet;
        Picture(int index, long due, VideoPacket packet) { this.index = index; this.due = due; this.packet = packet; }
    }

    private void decode(Surface surface, H264Config config, Listener listener) {
        MediaCodec codec = null;
        boolean started = false;
        try {
            MediaFormat format = MediaFormat.createVideoFormat("video/avc", width, height);
            format.setByteBuffer("csd-0", ByteBuffer.wrap(config.sps));
            format.setByteBuffer("csd-1", ByteBuffer.wrap(config.pps));
            format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 4 * 1024 * 1024);
            format.setInteger(MediaFormat.KEY_PRIORITY, 0);
            if (Build.VERSION.SDK_INT >= 30) format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1);
            codec = MediaCodec.createDecoderByType("video/avc");
            codec.configure(format, surface, null, 0); codec.start(); started = true; initialized = true;
            MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
            HashMap<Long, VideoPacket> submitted = new HashMap<>();
            ArrayDeque<Picture> pictures = new ArrayDeque<>();
            FramePacer pacer = new FramePacer();
            long timestamp = 0;
            DecoderWatchdog watchdog = new DecoderWatchdog();
            boolean first = true;
            while (!closed) {
                boolean worked = false, advanced = false;
                long now = System.nanoTime();
                // Release due pictures even while the socket is waiting for data.
                while (!pictures.isEmpty() && pictures.peekFirst().due <= now) {
                    Picture picture = pictures.removeFirst();
                    boolean newestDue = pictures.isEmpty() || pictures.peekFirst().due > now;
                    codec.releaseOutputBuffer(picture.index, newestDue);
                    if (newestDue && !closed) listener.output(picture.packet);
                    advanced = true; worked = true;
                }
                if (pictures.size() < 3) {
                    int index = codec.dequeueOutputBuffer(info, 0);
                    if (index >= 0) {
                        VideoPacket packet = submitted.remove(info.presentationTimeUs);
                        if (packet == null || (info.flags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) != 0) codec.releaseOutputBuffer(index, false);
                        else pictures.addLast(new Picture(index, pacer.due(info.presentationTimeUs, now), packet));
                        worked = true; advanced = true;
                    } else if (index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) worked = true;
                }
                VideoPacket packet = packets.peek();
                if (packet != null && submitted.size() < 8) {
                    int index = codec.dequeueInputBuffer(0);
                    if (index >= 0) {
                        packets.remove();
                        byte[] bytes = first ? config.frame : packet.bytes; first = false;
                        ByteBuffer input = codec.getInputBuffer(index);
                        if (input == null || input.capacity() < bytes.length) throw new IOException("Video packet too large");
                        input.clear(); input.put(bytes);
                        submitted.put(timestamp, packet);
                        codec.queueInputBuffer(index, 0, bytes.length, timestamp, 0);
                        timestamp += 33333; worked = true;
                    }
                }
                // Some codecs retain input until another picture arrives. A
                // silent socket is not evidence of a hung driver; apply the
                // decoder deadline only while fresh input is waiting to enter.
                if (watchdog.stalled(now, !packets.isEmpty(), advanced))
                    throw new IOException("Video decoder stalled");
                if (!worked) Thread.sleep(2);
            }
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt();
        } catch (Exception error) {
            // Buffer pressure is recoverable. Only codec creation/configuration
            // failure means that video is unavailable on this device.
            failure = started ? (error instanceof IOException ? (IOException) error : new IOException("Restart video decoder", error)) : new Unavailable();
        } finally {
            packets.clear();
            if (codec != null) { try { codec.release(); } catch (RuntimeException ignored) {} }
        }
    }

    @Override public void close() {
        closed = true; worker.interrupt();
        DecoderSlot.awaitStopped(worker, 1000);
    }
    boolean terminated() { return !worker.isAlive(); }
}
