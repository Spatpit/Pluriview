package io.pluriview.viewer;

import java.io.IOException;
import java.util.concurrent.TimeUnit;

/** One decoder per activity, including while an earlier session is shutting down. */
final class DecoderSlot {
    private Thread active;
    synchronized boolean available() { return active == null || !active.isAlive(); }
    synchronized void start(Thread worker) throws IOException {
        if (!available()) throw new IOException("Previous video decoder is stopping");
        active = worker;
        worker.start();
    }
    static boolean awaitStopped(Thread worker, long timeoutMillis) {
        long deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMillis);
        boolean interrupted = false;
        try {
            while (worker.isAlive()) {
                long remaining = deadline - System.nanoTime();
                if (remaining <= 0) return false;
                try { TimeUnit.NANOSECONDS.timedJoin(worker, remaining); }
                catch (InterruptedException stopped) { interrupted = true; }
            }
            return true;
        } finally {
            // Backgrounding interrupts the caller. Still allow bounded codec
            // cleanup, then restore cancellation for the outgoing session.
            if (interrupted) Thread.currentThread().interrupt();
        }
    }
}
