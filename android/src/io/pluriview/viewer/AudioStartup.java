package io.pluriview.viewer;

import java.io.IOException;
import java.io.InterruptedIOException;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Prepare the decoder before asking the PC to produce live audio. */
final class AudioStartup {
    private final CountDownLatch done = new CountDownLatch(1);
    private volatile IOException failure;
    synchronized void finish(IOException error) {
        if (done.getCount() == 0) return;
        failure = error; done.countDown();
    }
    void await() throws IOException {
        try {
            if (!done.await(3, TimeUnit.SECONDS)) throw new IOException("Audio decoder startup timed out");
        } catch (InterruptedException stopped) {
            Thread.currentThread().interrupt();
            throw new InterruptedIOException("Audio stopped");
        }
        if (failure != null) throw failure;
    }
}
