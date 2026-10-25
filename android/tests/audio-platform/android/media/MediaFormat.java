package android.media;
import java.nio.ByteBuffer;
import java.util.HashMap;
public final class MediaFormat {
    public static final String KEY_MAX_INPUT_SIZE = "max-input-size", KEY_SAMPLE_RATE = "sample-rate",
        KEY_CHANNEL_COUNT = "channel-count", KEY_PCM_ENCODING = "pcm-encoding";
    private final HashMap<String, Integer> values = new HashMap<>();
    public static MediaFormat createAudioFormat(String mime, int rate, int channels) {
        MediaFormat format = new MediaFormat();
        format.setInteger(KEY_SAMPLE_RATE, rate); format.setInteger(KEY_CHANNEL_COUNT, channels);
        return format;
    }
    public void setByteBuffer(String key, ByteBuffer value) {}
    public void setInteger(String key, int value) { values.put(key, value); }
    public int getInteger(String key) { return values.get(key); }
    public boolean containsKey(String key) { return values.containsKey(key); }
}
