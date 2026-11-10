//! Finding phones on the local network with DNS-SD (Bonjour).
//!
//! This uses the DNS-SD client built into Windows rather than its own multicast sockets, so it
//! needs no firewall rule: the queries go through the system's resolver service.

use std::net::SocketAddr;

use paircam_protocol::DeviceId;

/// A phone advertising the PairCam service on the network.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredCamera {
    /// The name the phone shows, such as "Ismail's iPhone".
    pub name: String,
    /// The phone's permanent id, when it advertises one.
    pub id: Option<DeviceId>,
    pub address: SocketAddr,
}

#[cfg(windows)]
pub use windows_impl::{Advertisement, Discovery};

#[cfg(not(windows))]
pub use unsupported::{Advertisement, Discovery};

/// Decodes a DNS-SD instance label, in which unusual bytes are written as `\DDD` and special
/// characters as `\c`.
fn unescape_label(label: &str) -> String {
    let bytes = label.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let digits = bytes.get(index + 1..index + 4);
        match digits.and_then(|digits| std::str::from_utf8(digits).ok()?.parse::<u8>().ok()) {
            Some(byte) => {
                decoded.push(byte);
                index += 4;
            }
            None => {
                if let Some(&next) = bytes.get(index + 1) {
                    decoded.push(next);
                }
                index += 2;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// The first label of a full instance name such as `My\032Phone._paircam._tcp.local`.
fn instance_label(full_name: &str) -> &str {
    let mut end = full_name.len();
    let bytes = full_name.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'.' => {
                end = index;
                break;
            }
            _ => index += 1,
        }
    }
    &full_name[..end.min(full_name.len())]
}

#[cfg(windows)]
mod windows_impl {
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Mutex;

    use paircam_protocol::DeviceId;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::Dns::{
        DNS_QUERY_REQUEST_VERSION1, DNS_RECORDW, DNS_SERVICE_BROWSE_REQUEST,
        DNS_SERVICE_BROWSE_REQUEST_0, DNS_SERVICE_CANCEL, DNS_SERVICE_INSTANCE,
        DNS_SERVICE_REGISTER_REQUEST, DNS_SERVICE_RESOLVE_REQUEST, DNS_TYPE_PTR, DnsFree,
        DnsFreeRecordList, DnsServiceBrowse, DnsServiceBrowseCancel, DnsServiceConstructInstance,
        DnsServiceFreeInstance, DnsServiceRegister, DnsServiceResolve,
    };
    use windows::core::{PCWSTR, PWSTR};

    use super::{DiscoveredCamera, instance_label, unescape_label};
    use crate::lock;

    /// `DNS_REQUEST_PENDING`: the request was accepted and its callback will be called.
    const PENDING: u32 = 9506;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Reads a NUL-terminated wide string owned by the system.
    unsafe fn read_wide(text: PWSTR) -> Option<String> {
        if text.is_null() {
            return None;
        }
        // SAFETY: the caller passes a valid NUL-terminated string.
        unsafe { text.to_string().ok() }
    }

    type OnChange = Box<dyn Fn(Vec<DiscoveredCamera>) + Send + Sync>;

    struct State {
        /// Cameras by their full instance name.
        found: Mutex<HashMap<String, DiscoveredCamera>>,
        on_change: OnChange,
    }

    impl State {
        fn report(&self) {
            let mut cameras: Vec<DiscoveredCamera> = lock(&self.found).values().cloned().collect();
            cameras.sort_by(|a, b| a.name.cmp(&b.name));
            (self.on_change)(cameras);
        }
    }

    /// A running search for phones. Results arrive through the callback given to [`start`].
    ///
    /// [`start`]: Discovery::start
    pub struct Discovery {
        cancel: DNS_SERVICE_CANCEL,
    }

    // SAFETY: the cancel handle is an opaque token the DNS API allows using from any thread.
    unsafe impl Send for Discovery {}
    unsafe impl Sync for Discovery {}

    impl Discovery {
        /// Starts looking for `service_type` (such as `_paircam._tcp`) and calls `on_change`
        /// with the full list every time a phone appears, changes or goes away.
        pub fn start(
            service_type: &str,
            on_change: impl Fn(Vec<DiscoveredCamera>) + Send + Sync + 'static,
        ) -> Option<Self> {
            // The state is shared with system callbacks that can still run after the search is
            // cancelled, so it is deliberately never freed. One search lasts the whole run.
            let state: &'static State = Box::leak(Box::new(State {
                found: Mutex::new(HashMap::new()),
                on_change: Box::new(on_change),
            }));
            let query: &'static [u16] =
                Box::leak(wide(&format!("{service_type}.local")).into_boxed_slice());

            let request = DNS_SERVICE_BROWSE_REQUEST {
                Version: DNS_QUERY_REQUEST_VERSION1.0,
                InterfaceIndex: 0,
                QueryName: PCWSTR(query.as_ptr()),
                Anonymous: DNS_SERVICE_BROWSE_REQUEST_0 {
                    pBrowseCallback: Some(browse_callback),
                },
                pQueryContext: std::ptr::from_ref(state).cast_mut().cast(),
            };
            let mut cancel = DNS_SERVICE_CANCEL::default();
            // SAFETY: the request and everything it points to outlive the search.
            let status = unsafe { DnsServiceBrowse(&request, &mut cancel) };
            if status as u32 != PENDING {
                log::warn!("cannot search the network for phones (status {status})");
                return None;
            }
            Some(Self { cancel })
        }
    }

    impl Drop for Discovery {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful DnsServiceBrowse.
            unsafe {
                DnsServiceBrowseCancel(&self.cancel);
            }
        }
    }

    unsafe extern "system" fn browse_callback(
        status: u32,
        context: *const c_void,
        records: *const DNS_RECORDW,
    ) {
        if status != 0 || records.is_null() {
            return;
        }
        // SAFETY: `context` is the leaked `State` passed to DnsServiceBrowse.
        let state = unsafe { &*context.cast::<State>() };

        let mut removed = false;
        let mut record = records;
        while !record.is_null() {
            // SAFETY: the system passes a valid, linked list of records.
            let current = unsafe { &*record };
            record = current.pNext;
            if current.wType != DNS_TYPE_PTR.0 {
                continue;
            }
            // SAFETY: a PTR record carries a host name in its data.
            let Some(instance) = (unsafe { read_wide(current.Data.PTR.pNameHost) }) else {
                continue;
            };
            if current.dwTtl == 0 {
                removed |= lock(&state.found).remove(&instance).is_some();
            } else {
                resolve(state, &instance);
            }
        }
        // SAFETY: the callback owns the record list and must free it.
        unsafe { DnsFree(Some(records.cast()), DnsFreeRecordList) };

        if removed {
            state.report();
        }
    }

    /// What a resolve request needs to keep alive until its callback runs.
    struct Resolving {
        state: &'static State,
        instance: String,
        query: Vec<u16>,
    }

    fn resolve(state: &'static State, instance: &str) {
        let resolving = Box::into_raw(Box::new(Resolving {
            state,
            instance: instance.to_owned(),
            query: wide(instance),
        }));
        // SAFETY: `resolving` is a valid allocation that the callback takes back.
        let request = DNS_SERVICE_RESOLVE_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1.0,
            InterfaceIndex: 0,
            QueryName: PWSTR(unsafe { (*resolving).query.as_mut_ptr() }),
            pResolveCompletionCallback: Some(resolve_callback),
            pQueryContext: resolving.cast(),
        };
        let mut cancel = DNS_SERVICE_CANCEL::default();
        // SAFETY: the request points at memory owned by `resolving`.
        let status = unsafe { DnsServiceResolve(&request, &mut cancel) };
        if status as u32 != PENDING {
            // The callback will not run, so take the allocation back here.
            // SAFETY: `resolving` came from Box::into_raw above and was not handed over.
            drop(unsafe { Box::from_raw(resolving) });
        }
    }

    unsafe extern "system" fn resolve_callback(
        status: u32,
        context: *const c_void,
        instance: *const DNS_SERVICE_INSTANCE,
    ) {
        // SAFETY: `context` is the `Resolving` allocation handed to DnsServiceResolve.
        let resolving = unsafe { Box::from_raw(context.cast_mut().cast::<Resolving>()) };
        if instance.is_null() {
            return;
        }
        if status == 0 {
            // SAFETY: the system passes a valid instance on success.
            if let Some(camera) = unsafe { camera_from(&*instance, &resolving.instance) } {
                // The search repeats, and a computer with several network adapters sees the
                // same phone once per adapter. Only a new phone or a renamed one is news.
                let mut found = lock(&resolving.state.found);
                let changed = found
                    .get(&resolving.instance)
                    .is_none_or(|known| known.name != camera.name || known.id != camera.id);
                if changed {
                    found.insert(resolving.instance.clone(), camera);
                    drop(found);
                    resolving.state.report();
                }
            }
        }
        // SAFETY: the callback owns the instance and must free it.
        unsafe { DnsServiceFreeInstance(instance) };
    }

    unsafe fn camera_from(
        instance: &DNS_SERVICE_INSTANCE,
        full_name: &str,
    ) -> Option<DiscoveredCamera> {
        if instance.ip4Address.is_null() || instance.wPort == 0 {
            return None;
        }
        // SAFETY: checked for null above. The address is stored in network byte order.
        let ip = Ipv4Addr::from(unsafe { *instance.ip4Address }.to_ne_bytes());

        let mut name = None;
        let mut id = None;
        for index in 0..instance.dwPropertyCount as usize {
            // SAFETY: `keys` and `values` hold `dwPropertyCount` entries each.
            let (key, value) = unsafe {
                (
                    read_wide(*instance.keys.add(index)),
                    read_wide(*instance.values.add(index)),
                )
            };
            match (key.as_deref(), value) {
                (Some("name"), Some(value)) if !value.is_empty() => name = Some(value),
                (Some("id"), Some(value)) => id = DeviceId::from_hex(&value),
                _ => {}
            }
        }

        Some(DiscoveredCamera {
            name: name.unwrap_or_else(|| unescape_label(instance_label(full_name))),
            id,
            address: SocketAddr::new(ip.into(), instance.wPort),
        })
    }

    /// A service this process advertises on the network. Used by the simulator.
    pub struct Advertisement;

    impl Advertisement {
        /// Advertises `instance_name` under `service_type` on `port` with the given TXT records,
        /// for as long as the process runs.
        pub fn start(
            service_type: &str,
            instance_name: &str,
            port: u16,
            properties: &[(&str, &str)],
        ) -> Option<Self> {
            let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "paircam".into());
            let service = wide(&format!("{instance_name}.{service_type}.local"));
            let host = wide(&format!("{host}.local"));
            let keys: Vec<Vec<u16>> = properties.iter().map(|(key, _)| wide(key)).collect();
            let values: Vec<Vec<u16>> = properties.iter().map(|(_, value)| wide(value)).collect();
            let key_pointers: Vec<PCWSTR> = keys.iter().map(|key| PCWSTR(key.as_ptr())).collect();
            let value_pointers: Vec<PCWSTR> =
                values.iter().map(|value| PCWSTR(value.as_ptr())).collect();

            // SAFETY: every pointer is valid for the call; the system copies what it keeps.
            let instance = unsafe {
                DnsServiceConstructInstance(
                    PCWSTR(service.as_ptr()),
                    PCWSTR(host.as_ptr()),
                    None,
                    None,
                    port,
                    0,
                    0,
                    properties.len() as u32,
                    key_pointers.as_ptr(),
                    value_pointers.as_ptr(),
                )
            };
            if instance.is_null() {
                return None;
            }

            // The registration lasts as long as the request does, so it is kept for the run.
            let request: &'static DNS_SERVICE_REGISTER_REQUEST =
                Box::leak(Box::new(DNS_SERVICE_REGISTER_REQUEST {
                    Version: DNS_QUERY_REQUEST_VERSION1.0,
                    InterfaceIndex: 0,
                    pServiceInstance: instance,
                    pRegisterCompletionCallback: Some(register_callback),
                    pQueryContext: std::ptr::null_mut(),
                    hCredentials: HANDLE::default(),
                    unicastEnabled: false.into(),
                }));
            // SAFETY: the request and its instance are valid and never freed.
            let status = unsafe { DnsServiceRegister(request, None) };
            if status != PENDING {
                log::warn!("cannot advertise on the network (status {status})");
                return None;
            }
            Some(Self)
        }
    }

    unsafe extern "system" fn register_callback(
        status: u32,
        _: *const c_void,
        instance: *const DNS_SERVICE_INSTANCE,
    ) {
        if status != 0 {
            log::warn!("advertising on the network failed (status {status})");
        }
        if !instance.is_null() {
            // SAFETY: the callback owns this copy of the instance.
            unsafe { DnsServiceFreeInstance(instance) };
        }
    }
}

#[cfg(not(windows))]
mod unsupported {
    use super::DiscoveredCamera;

    pub struct Discovery;

    impl Discovery {
        pub fn start(
            _: &str,
            _: impl Fn(Vec<DiscoveredCamera>) + Send + Sync + 'static,
        ) -> Option<Self> {
            None
        }
    }

    pub struct Advertisement;

    impl Advertisement {
        pub fn start(_: &str, _: &str, _: u16, _: &[(&str, &str)]) -> Option<Self> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_instance_label_from_a_full_name() {
        assert_eq!(
            instance_label("My\\032Phone._paircam._tcp.local"),
            "My\\032Phone"
        );
        assert_eq!(
            instance_label("Dotted\\.Name._paircam._tcp.local"),
            "Dotted\\.Name"
        );
        assert_eq!(instance_label("plain"), "plain");
    }

    #[test]
    fn decodes_escaped_labels() {
        assert_eq!(unescape_label("My\\032Phone"), "My Phone");
        assert_eq!(unescape_label("Dotted\\.Name"), "Dotted.Name");
        assert_eq!(
            unescape_label("Ismail\\226\\128\\153s\\032iPhone"),
            "Ismail\u{2019}s iPhone"
        );
        assert_eq!(unescape_label("plain"), "plain");
    }
}
