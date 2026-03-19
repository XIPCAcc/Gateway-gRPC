use std::os::fd::{AsRawFd, RawFd};

use nix::sys::eventfd::{eventfd, EfdFlags};
use nix::unistd::{read, write};

use crate::Result;

/// EventFD wrapper for signaling between processes
pub struct EventFd {
    fd: RawFd,
}

impl EventFd {
    /// Create a new EventFd
    pub fn new() -> Result<Self> {
        let fd = eventfd(0, EfdFlags::EFD_CLOEXEC | EfdFlags::EFD_NONBLOCK)?;
        Ok(Self { fd: fd.as_raw_fd() })
    }

    /// Create a new EventFd with semaphore semantics
    pub fn new_semaphore() -> Result<Self> {
        let fd = eventfd(
            0,
            EfdFlags::EFD_CLOEXEC | EfdFlags::EFD_NONBLOCK | EfdFlags::EFD_SEMAPHORE,
        )?;
        Ok(Self { fd: fd.as_raw_fd() })
    }

    /// Signal the eventfd (write 1)
    pub fn signal(&self) -> Result<()> {
        let buf: [u8; 8] = 1u64.to_ne_bytes();
        let _ = write(self.fd, &buf)?;
        Ok(())
    }

    /// Wait for signal (blocking)
    pub fn wait(&self) -> Result<u64> {
        let mut buf = [0u8; 8];
        let n = read(self.fd, &mut buf)?;
        if n == 8 {
            Ok(u64::from_ne_bytes(buf))
        } else {
            Err(crate::ShmError::InvalidState)
        }
    }

    /// Try to read without blocking
    pub fn try_wait(&self) -> Result<Option<u64>> {
        let mut buf = [0u8; 8];
        match read(self.fd, &mut buf) {
            Ok(8) => Ok(Some(u64::from_ne_bytes(buf))),
            Err(nix::errno::Errno::EAGAIN) => Ok(None),
            Err(e) => Err(e.into()),
            Ok(_) => Err(crate::ShmError::InvalidState),
        }
    }

    /// Clear all pending events
    pub fn clear(&self) -> Result<u64> {
        let mut buf = [0u8; 8];
        match read(self.fd, &mut buf) {
            Ok(8) => Ok(u64::from_ne_bytes(buf)),
            Err(nix::errno::Errno::EAGAIN) => Ok(0),
            Err(e) => Err(e.into()),
            Ok(_) => Err(crate::ShmError::InvalidState),
        }
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl AsRawFd for EventFd {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl Drop for EventFd {
    fn drop(&mut self) {
        let _ = nix::unistd::close(self.fd);
    }
}

/// Bi-directional signaling using two eventfds
pub struct EventFdPair {
    pub tx: EventFd,
    pub rx: EventFd,
}

impl EventFdPair {
    pub fn new() -> Result<Self> {
        Ok(Self {
            tx: EventFd::new()?,
            rx: EventFd::new()?,
        })
    }

    /// Split into sender and receiver ends
    pub fn split(self) -> (EventFdSender, EventFdReceiver) {
        (
            EventFdSender { notify: self.tx },
            EventFdReceiver { wait: self.rx },
        )
    }
}

pub struct EventFdSender {
    notify: EventFd,
}

impl EventFdSender {
    pub fn notify(&self) -> Result<()> {
        self.notify.signal()
    }
}

pub struct EventFdReceiver {
    wait: EventFd,
}

impl EventFdReceiver {
    pub fn wait(&self) -> Result<u64> {
        self.wait.wait()
    }

    pub fn try_wait(&self) -> Result<Option<u64>> {
        self.wait.try_wait()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eventfd() {
        let ev = EventFd::new().unwrap();
        ev.signal().unwrap();
        let count = ev.wait().unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_eventfd_semaphore() {
        let ev = EventFd::new_semaphore().unwrap();
        ev.signal().unwrap();
        ev.signal().unwrap();
        
        let count1 = ev.wait().unwrap();
        let count2 = ev.wait().unwrap();
        
        assert_eq!(count1, 1);
        assert_eq!(count2, 1);
    }
}
