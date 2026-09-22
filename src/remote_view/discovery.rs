//! Native Windows mDNS advertisement; never publish pairing credentials.
use super::*;
use std::ffi::c_void;
use windows::Win32::Foundation::DNS_REQUEST_PENDING;
use windows::Win32::NetworkManagement::{Dns::*, IpHelper::GetBestInterface};
use windows_core::PCWSTR;

unsafe extern "system" fn completed(
    status: u32,
    context: *const c_void,
    instance: *const DNS_SERVICE_INSTANCE,
) {
    if !instance.is_null() {
        DnsServiceFreeInstance(instance);
    }
    // Each asynchronous operation transfers one sender to its single callback.
    let sender = Box::from_raw(context as *mut mpsc::Sender<u32>);
    let _ = sender.send(status);
}

fn operation(instance: *mut DNS_SERVICE_INSTANCE, interface: u32, remove: bool) -> u32 {
    let (tx, rx) = mpsc::channel();
    let context = Box::into_raw(Box::new(tx));
    let request = DNS_SERVICE_REGISTER_REQUEST {
        Version: DNS_QUERY_REQUEST_VERSION1.0,
        InterfaceIndex: interface,
        pServiceInstance: instance,
        pRegisterCompletionCallback: Some(completed),
        pQueryContext: context.cast(),
        ..Default::default()
    };
    // This runs on the discovery worker, never on the UI or streaming workers.
    // The request and instance remain alive until Windows completes the call.
    let result = unsafe {
        if remove {
            DnsServiceDeRegister(&request, None)
        } else {
            DnsServiceRegister(&request, None)
        }
    };
    if result != DNS_REQUEST_PENDING as u32 {
        unsafe {
            drop(Box::from_raw(context));
        }
        return result;
    }
    rx.recv().unwrap_or(1)
}

pub(super) fn start(ip: Ipv4Addr, port: u16, shared: Arc<Shared>, ctx: egui::Context) {
    let status = shared.clone();
    if std::thread::Builder::new()
        .name("viewer-discovery".into())
        .spawn(move || {
            let wide = |value: &str| value.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            let label = shared.name.replace(' ', "-");
            let service = wide(&format!("{label}._pluriview._tcp.local"));
            // Windows only resolves its existing host name; an invented alias can
            // register successfully but leave phones unable to resolve the service.
            let Some(computer) = std::env::var("COMPUTERNAME").ok().filter(|name| {
                !name.is_empty()
                    && name.len() <= 63
                    && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            }) else {
                *shared.discovery_status.lock() =
                    "Local discovery unavailable. Use QR/link pairing.";
                ctx.request_repaint();
                return;
            };
            let hostname = wide(&format!("{computer}.local"));
            let key = wide("pv");
            let value = wide("3");
            let keys = [PCWSTR(key.as_ptr())];
            let values = [PCWSTR(value.as_ptr())];
            let address = u32::from_ne_bytes(ip.octets());
            let mut interface = 0;
            let instance = unsafe {
                // GetBestInterface for the bound local address selects that address's
                // interface. Never fall back to advertising on all interfaces.
                if GetBestInterface(address, &mut interface) == 0 && interface != 0 {
                    DnsServiceConstructInstance(
                        PCWSTR(service.as_ptr()),
                        PCWSTR(hostname.as_ptr()),
                        Some(&address),
                        None,
                        port,
                        0,
                        0,
                        1,
                        keys.as_ptr(),
                        values.as_ptr(),
                    )
                } else {
                    std::ptr::null_mut()
                }
            };
            if instance.is_null() {
                *shared.discovery_status.lock() =
                    "Local discovery unavailable. Use QR/link pairing.";
                ctx.request_repaint();
                return;
            }
            let result = operation(instance, interface, false);
            *shared.discovery_status.lock() = if result == 0 {
                "Visible to nearby phones on this network"
            } else {
                "Local discovery unavailable. Use QR/link pairing."
            };
            ctx.request_repaint();
            if result == 0 {
                while shared.running.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(200));
                }
                *shared.discovery_status.lock() = if operation(instance, interface, true) == 0 {
                    "Local discovery stopped"
                } else {
                    "Local discovery cleanup failed"
                };
            }
            unsafe {
                DnsServiceFreeInstance(instance);
            }
        })
        .is_err()
    {
        *status.discovery_status.lock() = "Local discovery unavailable. Use QR/link pairing.";
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "advertises a temporary service on the selected private LAN interface"]
    fn native_discovery_registers_resolves_and_stops() {
        // Keep native request storage alive on its worker if Windows never calls
        // back, while still bounding the opt-in test's duration.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(std::panic::catch_unwind(probe));
        });
        assert!(rx
            .recv_timeout(Duration::from_secs(40))
            .expect("Native discovery probe timed out")
            .is_ok());
    }

    fn probe() {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).unwrap();
        let std::net::IpAddr::V4(ip) = socket.local_addr().unwrap().ip() else {
            panic!("IPv4 needed");
        };
        assert!(ip.is_private());
        let host = Host::start(ip, egui::Context::default()).unwrap();
        let state = host.shared.clone();
        let until = Instant::now() + Duration::from_secs(15);
        while *state.discovery_status.lock() == "Local discovery starting…"
            && Instant::now() < until
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            *state.discovery_status.lock(),
            "Visible to nearby phones on this network"
        );
        eprintln!("Native registration completed; checking resolution.");
        let service: Vec<_> = format!("{}._pluriview._tcp.local", state.name.replace(' ', "-"))
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe extern "system" fn resolved(
            status: u32,
            context: *const c_void,
            instance: *const DNS_SERVICE_INSTANCE,
        ) {
            let mut result = (status, 0u32, 0u16);
            if !instance.is_null() {
                result.2 = (*instance).wPort;
                if !(*instance).ip4Address.is_null() {
                    result.1 = *(*instance).ip4Address;
                }
                DnsServiceFreeInstance(instance);
            }
            let tx = Box::from_raw(context as *mut mpsc::Sender<(u32, u32, u16)>);
            let _ = tx.send(result);
        }
        let (tx, rx) = mpsc::channel::<(u32, u32, u16)>();
        let context = Box::into_raw(Box::new(tx));
        let request = DNS_SERVICE_RESOLVE_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1.0,
            InterfaceIndex: {
                let mut index = 0;
                unsafe {
                    GetBestInterface(u32::from_ne_bytes(ip.octets()), &mut index);
                }
                index
            },
            QueryName: windows_core::PWSTR(service.as_ptr() as *mut _),
            pResolveCompletionCallback: Some(resolved),
            pQueryContext: context.cast(),
            ..Default::default()
        };
        let mut cancel = DNS_SERVICE_CANCEL::default();
        let status = unsafe { DnsServiceResolve(&request, &mut cancel) };
        assert_eq!(status, DNS_REQUEST_PENDING);
        let result = rx.recv().unwrap();
        assert_eq!(result.0, 0, "Registered service must resolve");
        assert_eq!(result.1, u32::from_ne_bytes(ip.octets()));
        let uri = url::Url::parse(&host.invite).unwrap();
        let port = uri
            .query_pairs()
            .find(|(key, _)| key == "port")
            .unwrap()
            .1
            .parse::<u16>()
            .unwrap();
        assert_eq!(result.2, port);
        drop(host);
        let until = Instant::now() + Duration::from_secs(15);
        while *state.discovery_status.lock() != "Local discovery stopped" && Instant::now() < until
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(*state.discovery_status.lock(), "Local discovery stopped");
        eprintln!(
            "Native mDNS registered, resolved to the selected interface/port, and deregistered."
        );
    }
}
