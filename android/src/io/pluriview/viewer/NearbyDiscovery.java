package io.pluriview.viewer;

import android.content.Context;
import android.net.nsd.NsdManager;
import android.net.nsd.NsdServiceInfo;
import android.net.wifi.WifiManager;
import android.os.Handler;
import android.os.Looper;
import java.net.Inet4Address;
import java.util.ArrayDeque;
import java.util.HashSet;

/** Platform DNS-SD, with bounded results and one legacy resolve call at a time. */
final class NearbyDiscovery implements AutoCloseable {
    interface Listener {
        void found(String key, String ip, int port);
        void lost(String key);
        void unavailable();
    }
    private final NsdManager manager;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final Listener listener;
    private final HashSet<String> seen = new HashSet<>();
    private final ArrayDeque<NsdServiceInfo> queue = new ArrayDeque<>();
    private boolean closed, resolving;
    private WifiManager.MulticastLock multicast;
    private final NsdManager.DiscoveryListener discovery = new NsdManager.DiscoveryListener() {
        public void onDiscoveryStarted(String type) {}
        public void onDiscoveryStopped(String type) {}
        public void onStartDiscoveryFailed(String type, int error) { main.post(() -> { releaseMulticast(); if (!closed) listener.unavailable(); }); }
        public void onStopDiscoveryFailed(String type, int error) {}
        public void onServiceFound(NsdServiceInfo info) { main.post(() -> {
            if (closed || seen.size() >= 16 || !(info.getServiceType().equals("_pluriview._tcp.") || info.getServiceType().equals("_pluriview._tcp"))) return;
            if (seen.add(info.getServiceName())) { queue.addLast(info); resolveNext(); }
        }); }
        public void onServiceLost(NsdServiceInfo info) { main.post(() -> {
            if (!closed) { seen.remove(info.getServiceName()); listener.lost(info.getServiceName()); }
        }); }
    };
    NearbyDiscovery(Context context, Listener listener) {
        this.listener = listener;
        manager = (NsdManager) context.getSystemService(Context.NSD_SERVICE);
        // Older Android Wi-Fi stacks require this even for foreground NSD.
        // The pairing screen releases it on stop; streaming never holds it.
        try {
            WifiManager wifi = (WifiManager) context.getApplicationContext().getSystemService(Context.WIFI_SERVICE);
            if (wifi != null) {
                multicast = wifi.createMulticastLock("PluriviewNearby");
                multicast.setReferenceCounted(false); multicast.acquire();
            }
        } catch (RuntimeException error) { releaseMulticast(); }
        try { manager.discoverServices("_pluriview._tcp.", NsdManager.PROTOCOL_DNS_SD, discovery); }
        catch (RuntimeException error) { releaseMulticast(); main.post(() -> { if (!closed) listener.unavailable(); }); }
    }
    private void resolveNext() {
        if (closed || resolving || queue.isEmpty()) return;
        NsdServiceInfo info = queue.removeFirst();
        if (!seen.contains(info.getServiceName())) { resolveNext(); return; }
        resolving = true;
        try { manager.resolveService(info, new NsdManager.ResolveListener() {
            public void onResolveFailed(NsdServiceInfo service, int error) { main.post(() -> { resolving = false; resolveNext(); }); }
            public void onServiceResolved(NsdServiceInfo service) { main.post(() -> {
                resolving = false;
                if (!closed && seen.contains(info.getServiceName()) && service.getHost() instanceof Inet4Address
                        && ViewerActivity.privateIpv4(service.getHost().getHostAddress()) && service.getPort() > 0 && service.getPort() <= 65535)
                    listener.found(info.getServiceName(), service.getHost().getHostAddress(), service.getPort());
                resolveNext();
            }); }
        }); } catch (RuntimeException error) { resolving = false; resolveNext(); }
    }
    @Override public void close() {
        closed = true; queue.clear(); seen.clear();
        try { manager.stopServiceDiscovery(discovery); } catch (RuntimeException ignored) {}
        releaseMulticast();
    }
    private void releaseMulticast() {
        if (multicast != null) {
            try { if (multicast.isHeld()) multicast.release(); } catch (RuntimeException ignored) {}
            multicast = null;
        }
    }
}
