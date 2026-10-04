package android.media;
import java.nio.ByteBuffer;

/** Bounded codec buffers exercise the production pump, without decoding AAC. */
public final class MediaCodec {
    public static final int INFO_TRY_AGAIN_LATER = -1, INFO_OUTPUT_FORMAT_CHANGED = -2;
    public static volatile boolean rejectInput;
    private int pending;
    private boolean formatSent, held;
    private ByteBuffer input = ByteBuffer.allocate(8192);
    public static final class BufferInfo { public int offset, size; }
    public static MediaCodec createDecoderByType(String type) { return new MediaCodec(); }
    public void configure(MediaFormat format, Object surface, Object crypto, int flags) {}
    public void start() {}
    public int dequeueInputBuffer(long timeout) { return !rejectInput && pending < 3 ? 0 : -1; }
    public ByteBuffer getInputBuffer(int index) { return input; }
    public void queueInputBuffer(int index, int offset, int size, long time, int flags) { pending++; }
    public int dequeueOutputBuffer(BufferInfo info, long timeout) {
        if (!formatSent) { formatSent = true; return INFO_OUTPUT_FORMAT_CHANGED; }
        if (held) throw new AssertionError("More than one output held");
        if (pending == 0) return INFO_TRY_AGAIN_LATER;
        held = true; info.offset = 0; info.size = 4096; return 0;
    }
    public MediaFormat getOutputFormat() { return MediaFormat.createAudioFormat("audio/raw", 48000, 2); }
    public ByteBuffer getOutputBuffer(int index) { return ByteBuffer.allocate(4096); }
    public void releaseOutputBuffer(int index, boolean render) {
        if (!held) throw new AssertionError("Output released twice");
        held = false; pending--;
    }
    public void stop() {}
    public void release() {}
}
