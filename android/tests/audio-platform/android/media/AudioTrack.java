package android.media;
import java.nio.ByteBuffer;

/** Small output with partial writes and deterministic transient pressure. */
public final class AudioTrack {
    public static final int MODE_STREAM = 1, WRITE_NON_BLOCKING = 1;
    public static volatile AudioTrack latest;
    public static volatile boolean stall;
    public static volatile long pressureMillis;
    public volatile int written;
    public volatile boolean playing, released;
    private long firstWrite;
    public static int getMinBufferSize(int rate, int channels, int encoding) { return 4096; }
    public static final class Builder {
        public Builder setAudioAttributes(AudioAttributes value) { return this; }
        public Builder setAudioFormat(AudioFormat value) { return this; }
        public Builder setBufferSizeInBytes(int value) { return this; }
        public Builder setTransferMode(int value) { return this; }
        public AudioTrack build() { latest = new AudioTrack(); return latest; }
    }
    public int write(ByteBuffer data, int size, int mode) {
        if (firstWrite == 0) firstWrite = System.nanoTime();
        if (stall || System.nanoTime() - firstWrite < pressureMillis * 1_000_000L) return 0;
        int count = Math.min(size, 1024);
        if (!playing) count = Math.min(count, Math.max(0, 4096 - written));
        data.position(data.position() + count); written += count;
        return count;
    }
    public void play() { playing = true; }
    public void pause() { playing = false; }
    public void flush() {}
    public void setVolume(float volume) {}
    public void release() { released = true; }
}
