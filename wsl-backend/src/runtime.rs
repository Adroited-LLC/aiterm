//! The small desktop-services interface needed by the shared Linux backend.
//! State and events are real; the Windows host owns the window and native IPC.
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    marker::PhantomData,
    ops::Deref,
    sync::{Arc, OnceLock},
};

pub mod async_runtime {
    fn handle() -> tokio::runtime::Handle {
        static FALLBACK: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
        tokio::runtime::Handle::try_current().unwrap_or_else(|_| {
            FALLBACK
                .get_or_init(|| tokio::runtime::Runtime::new().expect("workspace runtime"))
                .handle()
                .clone()
        })
    }
    pub struct JoinHandle<T>(tokio::task::JoinHandle<T>);
    impl<T> JoinHandle<T> {
        pub fn inner(&self) -> &tokio::task::JoinHandle<T> {
            &self.0
        }
        pub fn abort(&self) {
            self.0.abort();
        }
    }
    impl<T> std::future::Future for JoinHandle<T> {
        type Output = Result<T, tokio::task::JoinError>;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            std::pin::Pin::new(&mut self.get_mut().0).poll(cx)
        }
    }
    pub fn spawn<F: std::future::Future + Send + 'static>(f: F) -> JoinHandle<F::Output>
    where
        F::Output: Send + 'static,
    {
        JoinHandle(handle().spawn(f))
    }
    pub fn spawn_blocking<F: FnOnce() -> T + Send + 'static, T: Send + 'static>(
        f: F,
    ) -> JoinHandle<T> {
        JoinHandle(handle().spawn_blocking(f))
    }
}
type States = HashMap<TypeId, Arc<dyn Any + Send + Sync>>;
#[derive(Clone, Default)]
pub struct AppHandle {
    states: Arc<States>,
}
pub struct State<'a, T> {
    value: Arc<T>,
    lifetime: PhantomData<&'a T>,
}
impl<T> Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}
impl<T> State<'_, T> {
    pub fn inner(&self) -> &T {
        &self.value
    }
}
pub trait Manager {
    fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>>;
    fn state<T: Send + Sync + 'static>(&self) -> State<'_, T> {
        self.try_state()
            .expect("workspace state was not registered")
    }
}
impl Manager for AppHandle {
    fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>> {
        Some(State {
            value: self
                .states
                .get(&TypeId::of::<T>())?
                .clone()
                .downcast()
                .ok()?,
            lifetime: PhantomData,
        })
    }
}
impl AppHandle {
    pub fn manage<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        Arc::get_mut(&mut self.states)
            .expect("register state before cloning")
            .insert(TypeId::of::<T>(), Arc::new(value));
        self
    }
}
pub trait Emitter {
    fn emit<S: Serialize + Clone>(&self, name: &str, value: S) -> Result<(), String>;
}
impl Emitter for AppHandle {
    fn emit<S: Serialize + Clone>(&self, name: &str, value: S) -> Result<(), String> {
        send(json!({"type":"event","name":name,"payload":value}))
    }
}
type EventSink = Box<dyn Fn(Value) -> Result<(), String> + Send + Sync>;
static SINK: OnceLock<EventSink> = OnceLock::new();
pub fn install_sink(sink: EventSink) {
    assert!(
        SINK.set(sink).is_ok(),
        "only one workspace service per process"
    );
}
pub fn send(event: Value) -> Result<(), String> {
    SINK.get().ok_or("workspace transport closed")?(event)
}

pub mod ipc {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    pub enum InvokeResponseBody {
        Raw(Vec<u8>),
    }
    use std::sync::{Condvar, Mutex, Weak};
    #[derive(Default)]
    struct Credit {
        bytes: usize,
        closed: bool,
    }
    type Flow = (Mutex<Credit>, Condvar);
    static CHANNELS: OnceLock<Mutex<HashMap<u32, Weak<Flow>>>> = OnceLock::new();
    pub struct Channel<T> {
        id: u32,
        flow: Arc<Flow>,
        marker: PhantomData<T>,
    }
    impl<T> Drop for Channel<T> {
        fn drop(&mut self) {
            let _ = super::send(json!({"type":"channel_end","id":self.id}));
        }
    }
    pub fn control(request: &Value) {
        let Some(id) = request["channel"].as_u64() else {
            return;
        };
        if let Some(flow) = CHANNELS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .get(&(id as u32))
            .and_then(Weak::upgrade)
        {
            let mut credit = flow.0.lock().unwrap();
            if request["type"] == "channel_close" {
                credit.closed = true;
            } else if let Some(bytes) = request["bytes"].as_u64() {
                credit.bytes = credit.bytes.saturating_sub(bytes as usize);
            }
            flow.1.notify_all();
        }
    }
    pub fn close_all() {
        for flow in CHANNELS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .values()
            .filter_map(Weak::upgrade)
        {
            flow.0.lock().unwrap().closed = true;
            flow.1.notify_all();
        }
    }
    impl<'de, T> serde::Deserialize<'de> for Channel<T> {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let raw = <String as serde::Deserialize>::deserialize(d)?;
            let id = raw
                .strip_prefix("__CHANNEL__:")
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| serde::de::Error::custom("invalid desktop channel"))?;
            let flow = Arc::new((Mutex::new(Credit::default()), Condvar::new()));
            let mut channels = CHANNELS.get_or_init(Default::default).lock().unwrap();
            channels.retain(|_, flow| flow.strong_count() > 0);
            channels.insert(id, Arc::downgrade(&flow));
            Ok(Self {
                id,
                flow,
                marker: PhantomData,
            })
        }
    }
    impl Channel<InvokeResponseBody> {
        pub fn send(&self, value: InvokeResponseBody) -> Result<(), String> {
            let InvokeResponseBody::Raw(bytes) = value;
            for chunk in bytes.chunks(32 * 1024) {
                let credit = self.flow.0.lock().unwrap();
                let (mut credit, timeout) = self
                    .flow
                    .1
                    .wait_timeout_while(credit, std::time::Duration::from_secs(30), |c| {
                        !c.closed && c.bytes + chunk.len() > aiterm_wsl_protocol::OUTPUT_WINDOW
                    })
                    .unwrap();
                if credit.closed || timeout.timed_out() {
                    return Err("Desktop terminal stopped consuming output".into());
                }
                credit.bytes += chunk.len();
                drop(credit);
                super::send(json!({"type":"channel","id":self.id,"data":STANDARD.encode(chunk)}))?;
            }
            Ok(())
        }
    }
}
