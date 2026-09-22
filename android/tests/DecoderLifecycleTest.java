package io.pluriview.viewer;

import java.io.IOException;
import java.util.concurrent.CountDownLatch;

public final class DecoderLifecycleTest {
    private static void check(boolean value, String reason) { if (!value) throw new AssertionError(reason); }
    public static void main(String[] args) throws Exception {
        DecoderWatchdog watchdog = new DecoderWatchdog();
        check(!watchdog.stalled(0, true, true), "First output stalled");
        check(!watchdog.stalled(33_000_000L, false, false), "Idle decoder stalled");
        check(!watchdog.stalled(2_000_000_000L, true, false), "Network idle time counted as a decoder stall");
        check(!watchdog.stalled(2_700_000_000L, true, false), "New work lost its timeout budget");
        check(watchdog.stalled(2_751_000_000L, true, false), "Stuck decoder did not time out");
        check(!watchdog.stalled(2_800_000_000L, true, true), "Output did not reset the watchdog");
        RecoveryWindow recovery = new RecoveryWindow();
        recovery.begin(0);
        check(!recovery.expired(500_000_000L), "Brief reconnect cleared the picture");
        recovery.begin(800_000_000L);
        check(recovery.expired(RecoveryWindow.HOLD_NANOS), "Repeated failures retained a stale picture indefinitely");
        recovery.reset();
        check(!recovery.expired(3_000_000_000L), "Old recovery timer could hide a resumed stream");
        recovery.begin(4_000_000_000L);
        check(!recovery.expired(4_100_000_000L), "New recovery inherited an old deadline");
        DecoderSlot slot = new DecoderSlot();
        for (int cycle = 0; cycle < 25; cycle++) {
            CountDownLatch release = new CountDownLatch(1);
            Thread decoder = new Thread(() -> {
                try { release.await(); } catch (InterruptedException ignored) {}
            });
            slot.start(decoder);
            try {
                slot.start(new Thread(() -> {}));
                throw new AssertionError("Overlapping resume started a second decoder");
            } catch (IOException expected) {}
            // Reproduce shutdownNow interrupting the stream worker while its
            // decoder still needs time to release its driver resources.
            Thread releaser = new Thread(() -> {
                try { Thread.sleep(15); } catch (InterruptedException ignored) {}
                release.countDown();
            });
            releaser.start();
            Thread.currentThread().interrupt();
            check(DecoderSlot.awaitStopped(decoder, 1000), "Background interruption abandoned decoder cleanup");
            check(Thread.interrupted(), "Outgoing session cancellation was lost");
            releaser.join();
            check(slot.available(), "Resume remained locked after cleanup");
        }
        CountDownLatch release = new CountDownLatch(1);
        Thread stuck = new Thread(() -> { try { release.await(); } catch (InterruptedException ignored) {} });
        slot.start(stuck);
        try {
            long start = System.nanoTime();
            check(!DecoderSlot.awaitStopped(stuck, 30), "Hung driver reported stopped");
            check(System.nanoTime() - start < 1_000_000_000L, "Driver cleanup wait was unbounded");
            check(!slot.available(), "Hung driver allowed another decoder");
        } finally { release.countDown(); stuck.join(1000); }
        check(slot.available(), "Recovered driver did not release its slot");
        FrameRate rate = new FrameRate(); rate.record(0); rate.record(33_333_333L);
        rate.clear(); check(rate.fps(33_333_333L) == 0, "Resume retained old stream statistics");
        System.out.println("Decoder background/resume, interrupted cleanup and bounded driver tests passed");
    }
}
