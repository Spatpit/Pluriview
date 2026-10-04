package io.pluriview.viewer;

import java.io.IOException;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/** JVM-only decoder stand-in. Never compiled into the APK. */
final class AudioDecoder implements AutoCloseable {
    static volatile CountDownLatch prepare = new CountDownLatch(1);
    static final AtomicInteger received = new AtomicInteger();
    private final AudioStartup startup = new AudioStartup();
    private final CountDownLatch stop = new CountDownLatch(1);
    private final Thread worker;
    AudioDecoder(DecoderSlot slot) throws IOException {
        worker = new Thread(() -> {
            try {
                if (!prepare.await(5, TimeUnit.SECONDS)) throw new IOException("Fixture timed out");
                startup.finish(null);
                stop.await();
            } catch (Exception error) { startup.finish(new IOException("Audio stopped")); }
        });
        slot.start(worker);
    }
    void awaitReady() throws IOException { startup.await(); }
    void queue(AudioPacket packet) { received.incrementAndGet(); }
    @Override public void close() {
        startup.finish(new IOException("Audio stopped")); stop.countDown(); worker.interrupt();
    }
}
