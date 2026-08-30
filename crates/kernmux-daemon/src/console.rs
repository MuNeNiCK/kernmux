//! Exclusive, binary-safe proxy for Multikernel MKTTY consoles.

use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use kernmux_api::v1::{
    ApiError, ConsoleAttachment, ConsoleCapabilities, ConsoleCloseReason, ConsoleSize, ErrorCode,
    InstanceId,
};

/// Largest binary payload accepted in one management-protocol frame.
pub const DEFAULT_MAX_CONSOLE_FRAME_BYTES: usize = 64 * 1024;

const MAX_INSTANCE_ID: u32 = 511;

/// Opens a fresh host-side MKTTY connection.
pub trait ConsoleDeviceFactory {
    type Device: Read + Write + Send + 'static;

    /// Opens the privileged device retained by the daemon.
    ///
    /// # Errors
    ///
    /// Returns the host device error without exposing it to an unprivileged
    /// client.
    fn open(&self) -> io::Result<Self::Device>;
}

/// Factory for the host kernel's `/dev/mktty` device.
#[derive(Clone, Debug)]
pub struct MkttyDeviceFactory {
    path: PathBuf,
}

impl MkttyDeviceFactory {
    /// Uses the running host's conventional MKTTY device.
    #[must_use]
    pub fn running_host() -> Self {
        Self::new("/dev/mktty")
    }

    /// Uses an explicit MKTTY-compatible device path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Device path opened by this factory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl ConsoleDeviceFactory for MkttyDeviceFactory {
    type Device = File;

    fn open(&self) -> io::Result<Self::Device> {
        OpenOptions::new().read(true).write(true).open(&self.path)
    }
}

/// Stable category for console failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleErrorKind {
    InvalidInstance,
    AlreadyAttached,
    FrameTooLarge,
    ResizeUnsupported,
    Closed(ConsoleCloseReason),
    DeviceUnavailable,
    DeviceIo,
}

/// Console failure with a stable public category and optional private source.
#[derive(Debug)]
pub struct ConsoleError {
    kind: ConsoleErrorKind,
    source: Option<io::Error>,
}

impl ConsoleError {
    fn stable(kind: ConsoleErrorKind) -> Self {
        Self { kind, source: None }
    }

    fn device(error: io::Error) -> Self {
        let kind = if matches!(error.kind(), io::ErrorKind::NotFound)
            || error.raw_os_error() == Some(19)
        {
            ConsoleErrorKind::DeviceUnavailable
        } else {
            ConsoleErrorKind::DeviceIo
        };
        Self {
            kind,
            source: Some(error),
        }
    }

    /// Stable failure category suitable for control-flow decisions.
    #[must_use]
    pub const fn kind(&self) -> ConsoleErrorKind {
        self.kind
    }

    /// Privileged host error retained for daemon-side diagnostics.
    #[must_use]
    pub const fn source_io(&self) -> Option<&io::Error> {
        self.source.as_ref()
    }

    /// Redacted error suitable for the management API.
    #[must_use]
    pub fn api_error(&self) -> ApiError {
        let (code, message, retryable) = match self.kind {
            ConsoleErrorKind::InvalidInstance => {
                (ErrorCode::InvalidRequest, "invalid console instance", false)
            }
            ConsoleErrorKind::AlreadyAttached => (
                ErrorCode::Conflict,
                "instance console already has an attached client",
                true,
            ),
            ConsoleErrorKind::FrameTooLarge => (
                ErrorCode::InvalidRequest,
                "console frame exceeds its limit",
                false,
            ),
            ConsoleErrorKind::ResizeUnsupported => (
                ErrorCode::Unsupported,
                "console resize is not supported by this host transport",
                false,
            ),
            ConsoleErrorKind::Closed(_) => {
                (ErrorCode::Conflict, "console session is closed", false)
            }
            ConsoleErrorKind::DeviceUnavailable => (
                ErrorCode::BackendUnavailable,
                "console device is unavailable",
                true,
            ),
            ConsoleErrorKind::DeviceIo => (
                ErrorCode::BackendUnavailable,
                "console device operation failed",
                true,
            ),
        };
        ApiError {
            code,
            message: message.into(),
            retryable,
            current_generation: None,
            diagnostics: Vec::new(),
        }
    }
}

impl std::fmt::Display for ConsoleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.api_error().message.fmt(formatter)
    }
}

impl std::error::Error for ConsoleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|error| error as &(dyn std::error::Error + 'static))
    }
}

/// One bounded read from a console stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConsoleRead {
    Data(Vec<u8>),
    Closed(ConsoleCloseReason),
}

/// Coordinates exclusive attachment to host MKTTY consoles.
#[derive(Debug)]
pub struct ConsoleProxy<F> {
    factory: F,
    attached: Arc<Mutex<BTreeSet<InstanceId>>>,
    max_frame_bytes: usize,
}

impl<F> ConsoleProxy<F>
where
    F: ConsoleDeviceFactory,
{
    /// Creates a proxy with a 64 KiB management frame limit.
    #[must_use]
    pub fn new(factory: F) -> Self {
        Self {
            factory,
            attached: Arc::new(Mutex::new(BTreeSet::new())),
            max_frame_bytes: DEFAULT_MAX_CONSOLE_FRAME_BYTES,
        }
    }

    /// Creates a proxy with an explicit nonzero frame limit.
    ///
    /// # Errors
    ///
    /// Rejects zero and limits that cannot be represented by the API.
    pub fn with_frame_limit(factory: F, max_frame_bytes: usize) -> Result<Self, ConsoleError> {
        if max_frame_bytes == 0 || u32::try_from(max_frame_bytes).is_err() {
            return Err(ConsoleError::stable(ConsoleErrorKind::FrameTooLarge));
        }
        Ok(Self {
            factory,
            attached: Arc::new(Mutex::new(BTreeSet::new())),
            max_frame_bytes,
        })
    }

    /// Opens and selects one active instance's host console.
    ///
    /// The caller must verify that the authoritative instance state is active
    /// before attaching. A second attachment to the same instance is rejected
    /// because the current MKTTY host driver delivers output to one reader.
    ///
    /// # Errors
    ///
    /// Rejects invalid IDs, attachment conflicts, and device failures.
    pub fn attach(
        &self,
        instance_id: InstanceId,
    ) -> Result<ConsoleSession<F::Device>, ConsoleError> {
        if instance_id.0 == 0 || instance_id.0 > MAX_INSTANCE_ID {
            return Err(ConsoleError::stable(ConsoleErrorKind::InvalidInstance));
        }
        let lease = ConsoleLease::acquire(Arc::clone(&self.attached), instance_id)?;
        let mut device = self.factory.open().map_err(ConsoleError::device)?;
        write_selector(&mut device, format!("{}\n", instance_id.0).as_bytes())
            .map_err(ConsoleError::device)?;
        let max_frame_bytes = u32::try_from(self.max_frame_bytes)
            .map_err(|_| ConsoleError::stable(ConsoleErrorKind::FrameTooLarge))?;
        Ok(ConsoleSession {
            attachment: ConsoleAttachment {
                instance_id,
                capabilities: ConsoleCapabilities {
                    binary: true,
                    resize: false,
                },
                max_frame_bytes,
            },
            device: Some(device),
            lease: Some(lease),
            closed: None,
        })
    }
}

#[derive(Debug)]
struct ConsoleLease {
    attached: Arc<Mutex<BTreeSet<InstanceId>>>,
    instance_id: InstanceId,
}

impl ConsoleLease {
    fn acquire(
        attached: Arc<Mutex<BTreeSet<InstanceId>>>,
        instance_id: InstanceId,
    ) -> Result<Self, ConsoleError> {
        let inserted = attached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(instance_id);
        if !inserted {
            return Err(ConsoleError::stable(ConsoleErrorKind::AlreadyAttached));
        }
        Ok(Self {
            attached,
            instance_id,
        })
    }
}

impl Drop for ConsoleLease {
    fn drop(&mut self) {
        self.attached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.instance_id);
    }
}

/// Exclusive binary console session. Dropping it always releases the lease.
#[derive(Debug)]
pub struct ConsoleSession<D> {
    attachment: ConsoleAttachment,
    device: Option<D>,
    lease: Option<ConsoleLease>,
    closed: Option<ConsoleCloseReason>,
}

impl<D> ConsoleSession<D>
where
    D: Read + Write,
{
    /// Negotiated metadata for this stream.
    #[must_use]
    pub const fn attachment(&self) -> ConsoleAttachment {
        self.attachment
    }

    /// Reads one bounded binary frame.
    ///
    /// # Errors
    ///
    /// Returns a redacted device failure after closing the session.
    pub fn read_frame(&mut self) -> Result<ConsoleRead, ConsoleError> {
        if let Some(reason) = self.closed {
            return Ok(ConsoleRead::Closed(reason));
        }
        let mut data = vec![0; self.attachment.max_frame_bytes as usize];
        let device = self.device.as_mut().ok_or_else(|| {
            ConsoleError::stable(ConsoleErrorKind::Closed(
                ConsoleCloseReason::DeviceUnavailable,
            ))
        })?;
        let read = loop {
            match device.read(&mut data) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                result => break result,
            }
        };
        match read {
            Ok(0) => Ok(ConsoleRead::Closed(
                self.close(ConsoleCloseReason::EndOfStream),
            )),
            Ok(bytes) => {
                data.truncate(bytes);
                Ok(ConsoleRead::Data(data))
            }
            Err(error) => {
                let reason = close_reason_for_io(&error);
                self.close(reason);
                Err(ConsoleError::device(error))
            }
        }
    }

    /// Writes one binary frame, retrying interrupted and partial writes.
    ///
    /// # Errors
    ///
    /// Rejects oversized frames and writes after closure. Device failures
    /// close the stream before being returned.
    pub fn write_frame(&mut self, data: &[u8]) -> Result<(), ConsoleError> {
        if data.len() > self.attachment.max_frame_bytes as usize {
            return Err(ConsoleError::stable(ConsoleErrorKind::FrameTooLarge));
        }
        if let Some(reason) = self.closed {
            return Err(ConsoleError::stable(ConsoleErrorKind::Closed(reason)));
        }
        let device = self.device.as_mut().ok_or_else(|| {
            ConsoleError::stable(ConsoleErrorKind::Closed(
                ConsoleCloseReason::DeviceUnavailable,
            ))
        })?;
        if let Err(error) = write_all_retry(device, data) {
            let reason = close_reason_for_io(&error);
            self.close(reason);
            return Err(ConsoleError::device(error));
        }
        Ok(())
    }

    /// Reports that resize is unavailable on the MKTTY transport.
    ///
    /// # Errors
    ///
    /// Always returns `ResizeUnsupported` without writing to the device.
    pub fn resize(&mut self, _size: ConsoleSize) -> Result<(), ConsoleError> {
        Err(ConsoleError::stable(ConsoleErrorKind::ResizeUnsupported))
    }

    /// Closes the stream after authoritative inventory observes instance stop.
    pub fn instance_stopped(&mut self) -> ConsoleCloseReason {
        self.close(ConsoleCloseReason::InstanceStopped)
    }

    /// Detaches the client and closes the privileged device.
    pub fn detach(&mut self) -> ConsoleCloseReason {
        self.close(ConsoleCloseReason::ClientDetached)
    }

    /// Current terminal reason, if closed.
    #[must_use]
    pub const fn close_reason(&self) -> Option<ConsoleCloseReason> {
        self.closed
    }

    fn close(&mut self, reason: ConsoleCloseReason) -> ConsoleCloseReason {
        if let Some(existing) = self.closed {
            return existing;
        }
        self.device.take();
        self.lease.take();
        self.closed = Some(reason);
        reason
    }
}

fn write_selector(writer: &mut impl Write, data: &[u8]) -> io::Result<()> {
    loop {
        match writer.write(data) {
            Ok(written) if written == data.len() => return Ok(()),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "MKTTY instance selector was not accepted atomically",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn write_all_retry(writer: &mut impl Write, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        match writer.write(data) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => data = &data[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn close_reason_for_io(error: &io::Error) -> ConsoleCloseReason {
    if matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::UnexpectedEof
    ) || matches!(error.raw_os_error(), Some(19 | 107))
    {
        ConsoleCloseReason::InstanceStopped
    } else if error.kind() == io::ErrorKind::NotFound {
        ConsoleCloseReason::DeviceUnavailable
    } else {
        ConsoleCloseReason::TransportError
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, io::Cursor};

    use super::*;

    #[derive(Clone, Debug)]
    struct FakeFactory {
        devices: Arc<Mutex<VecDeque<FakeDevice>>>,
    }

    impl FakeFactory {
        fn new(devices: impl IntoIterator<Item = FakeDevice>) -> Self {
            Self {
                devices: Arc::new(Mutex::new(devices.into_iter().collect())),
            }
        }
    }

    impl ConsoleDeviceFactory for FakeFactory {
        type Device = FakeDevice;

        fn open(&self) -> io::Result<Self::Device> {
            self.devices
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    #[derive(Debug)]
    struct FakeDevice {
        input: Cursor<Vec<u8>>,
        output: Arc<Mutex<Vec<u8>>>,
        max_write: usize,
        first_write: bool,
    }

    impl FakeDevice {
        fn new(input: Vec<u8>, output: Arc<Mutex<Vec<u8>>>, max_write: usize) -> Self {
            Self {
                input: Cursor::new(input),
                output,
                max_write,
                first_write: true,
            }
        }
    }

    impl Read for FakeDevice {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.input.read(buffer)
        }
    }

    impl Write for FakeDevice {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            let bytes = if self.first_write {
                self.first_write = false;
                data.len()
            } else {
                data.len().min(self.max_write)
            };
            self.output
                .lock()
                .unwrap()
                .extend_from_slice(&data[..bytes]);
            Ok(bytes)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn device(input: Vec<u8>, max_write: usize) -> (FakeDevice, Arc<Mutex<Vec<u8>>>) {
        let output = Arc::new(Mutex::new(Vec::new()));
        (
            FakeDevice::new(input, Arc::clone(&output), max_write),
            output,
        )
    }

    #[test]
    fn attach_selects_instance_and_streams_binary_through_partial_writes() {
        let (device, output) = device(vec![0, 0xff, b'\n'], 1);
        let proxy = ConsoleProxy::with_frame_limit(FakeFactory::new([device]), 8).unwrap();
        let mut session = proxy.attach(InstanceId(7)).unwrap();

        assert_eq!(output.lock().unwrap().as_slice(), b"7\n");
        assert_eq!(
            session.read_frame().unwrap(),
            ConsoleRead::Data(vec![0, 0xff, b'\n'])
        );
        session.write_frame(&[0xfe, 0, b'\r']).unwrap();
        assert_eq!(
            output.lock().unwrap().as_slice(),
            [b'7', b'\n', 0xfe, 0, b'\r']
        );
    }

    #[test]
    fn one_instance_has_one_attachment_until_raii_detach() {
        let (first, _) = device(Vec::new(), 8);
        let (second, _) = device(Vec::new(), 8);
        let proxy = ConsoleProxy::new(FakeFactory::new([first, second]));
        let mut session = proxy.attach(InstanceId(1)).unwrap();

        let conflict = proxy.attach(InstanceId(1)).unwrap_err();
        assert_eq!(conflict.kind(), ConsoleErrorKind::AlreadyAttached);
        assert_eq!(conflict.api_error().code, ErrorCode::Conflict);

        assert_eq!(session.detach(), ConsoleCloseReason::ClientDetached);
        assert!(proxy.attach(InstanceId(1)).is_ok());
    }

    #[test]
    fn resize_is_explicitly_unsupported_and_never_reaches_device() {
        let (device, output) = device(Vec::new(), 8);
        let proxy = ConsoleProxy::new(FakeFactory::new([device]));
        let mut session = proxy.attach(InstanceId(1)).unwrap();

        assert!(!session.attachment().capabilities.resize);
        let error = session
            .resize(ConsoleSize {
                columns: 120,
                rows: 40,
            })
            .unwrap_err();

        assert_eq!(error.kind(), ConsoleErrorKind::ResizeUnsupported);
        assert_eq!(error.api_error().code, ErrorCode::Unsupported);
        assert_eq!(output.lock().unwrap().as_slice(), b"1\n");
    }

    #[test]
    fn eof_and_authoritative_instance_stop_have_stable_close_reasons() {
        let (eof_device, _) = device(Vec::new(), 8);
        let (stop_device, _) = device(Vec::new(), 8);
        let proxy = ConsoleProxy::new(FakeFactory::new([eof_device, stop_device]));

        let mut eof = proxy.attach(InstanceId(1)).unwrap();
        assert_eq!(
            eof.read_frame().unwrap(),
            ConsoleRead::Closed(ConsoleCloseReason::EndOfStream)
        );
        drop(eof);

        let mut stopped = proxy.attach(InstanceId(1)).unwrap();
        assert_eq!(
            stopped.instance_stopped(),
            ConsoleCloseReason::InstanceStopped
        );
        assert_eq!(
            stopped.read_frame().unwrap(),
            ConsoleRead::Closed(ConsoleCloseReason::InstanceStopped)
        );
        assert_eq!(
            stopped.write_frame(b"ignored").unwrap_err().kind(),
            ConsoleErrorKind::Closed(ConsoleCloseReason::InstanceStopped)
        );
    }

    #[test]
    fn detach_is_idempotent_and_oversized_frames_are_rejected() {
        let (device, _) = device(Vec::new(), 8);
        let proxy = ConsoleProxy::with_frame_limit(FakeFactory::new([device]), 2).unwrap();
        let mut session = proxy.attach(InstanceId(1)).unwrap();

        assert_eq!(session.detach(), ConsoleCloseReason::ClientDetached);
        assert_eq!(session.detach(), ConsoleCloseReason::ClientDetached);
        assert_eq!(
            session.write_frame(&[1, 2, 3]).unwrap_err().kind(),
            ConsoleErrorKind::FrameTooLarge
        );
    }
}
