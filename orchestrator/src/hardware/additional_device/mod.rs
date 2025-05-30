use std::{fmt, iter, mem, sync::Arc};

use async_trait::async_trait;
use bytes::Buf;
use eyre::{Result, bail};
use tokio::{net::UdpSocket, sync::Mutex, task::JoinHandle, time::Instant};

use crate::{
    config::DeviceConfig,
    data::sink::{DataSinkBuilder, StreamWriter},
};

use super::HardwareAgent;

const BUFFER_SIZE: usize = 1024;
const MAGIC_NUMBER: u8 = 0xDE;

pub struct Device {
    inner: Arc<Inner>,
    task: JoinHandle<Result<()>>,
}

struct Inner {
    config: DeviceConfig,
    socket: UdpSocket,
    stream: Mutex<Option<StreamWriter>>,
}

impl Device {
    pub async fn connect_from_config(config: &DeviceConfig) -> Result<Self> {
        Self::connect(config.clone()).await
    }

    pub async fn connect(config: DeviceConfig) -> Result<Self> {
        let socket = UdpSocket::bind((config.ip, 0)).await?;

        socket.connect((config.ip, config.port)).await?;

        let inner = Arc::new(Inner::new(config, socket));
        let task = tokio::spawn(device_task(inner.clone()));

        Ok(Device { inner, task })
    }

    pub fn config(&self) -> &DeviceConfig {
        &self.inner.config
    }
}

#[tracing::instrument(skip_all, fields(device = inner.config.name))]
async fn device_task(inner: Arc<Inner>) -> Result<()> {
    let mut buf = Vec::with_capacity(BUFFER_SIZE);
    let mut data = Vec::with_capacity(inner.config.channels.len());

    loop {
        inner.socket.recv_buf(&mut buf).await?;
        let now = Instant::now();

        if let Some(ref stream) = *inner.stream.lock().await {
            let msg = parse_message(&buf[..], inner.config.channels.len())?;

            if let Message::State(mut payload) = msg {
                data.clear();
                data.extend(iter::from_fn(|| payload.try_get_f32().ok()));

                stream.add(now, &data).await;
            }
        }

        buf.clear();
    }
}

#[async_trait]
impl HardwareAgent for Device {
    async fn register(&mut self, sink: &mut DataSinkBuilder) {
        let DeviceConfig { name, channels, .. } = &self.inner.config;
        let stream = sink.register_stream(name.clone(), channels.clone()).await;

        *self.inner.stream.lock().await = Some(stream);
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Device ({})", self.inner.config.name)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Inner {
    fn new(config: DeviceConfig, socket: UdpSocket) -> Self {
        Self {
            config,
            socket,
            stream: Default::default(),
        }
    }
}

enum Message<B: Buf> {
    State(B),
    Error(u8),
}

fn parse_message<B: Buf>(mut buf: B, n_channels: usize) -> Result<Message<B>> {
    if buf.try_get_u8() != Ok(MAGIC_NUMBER) {
        bail!("Missing magic header!");
    }

    match buf.try_get_u8() {
        Ok(0x0) => {
            let expected_payload_size = n_channels * mem::size_of::<f32>();
            let buf_size = buf.remaining();

            // Check the message payload size is correct
            if buf_size != expected_payload_size {
                bail!("Expected {n_channels} floats ({expected_payload_size} B), got {buf_size}",);
            }

            Ok(Message::State(buf))
        }

        Ok(0x1) => Ok(Message::Error(buf.try_get_u8().unwrap_or(0xFF))),
        Ok(code) => bail!("Unsupported code: {code}"),
        Err(_) => bail!("Missing code!"),
    }
}
