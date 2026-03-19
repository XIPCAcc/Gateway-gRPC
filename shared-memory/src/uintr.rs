//! UINTR (User Interrupt) support for ultra-low latency IPC
//! 
//! UINTR is a relatively new x86 feature (Intel Sapphire Rapids+)
//! that allows user-space interrupts without kernel involvement.

use crate::Result;

/// UINTR handler function type
pub type UintrHandler = extern "C" fn(uintr_vec: u8, uintr_rc: u64);

/// UINTR vector configuration
#[derive(Debug, Clone, Copy)]
pub struct UintrConfig {
    pub vector: u8,
    pub flags: u64,
}

impl Default for UintrConfig {
    fn default() -> Self {
        Self {
            vector: 0,
            flags: 0,
        }
    }
}

/// Check if UINTR is supported on this CPU
pub fn is_uintr_supported() -> bool {
    // Check CPUID for UINTR support (CPUID leaf 7, ECX bit 5)
    // This is a simplified check - real implementation would use CPUID instruction
    false
}

/// UINTR sender (can send interrupts to a target)
pub struct UintrSender {
    target_id: u64,
}

impl UintrSender {
    /// Create a new UINTR sender
    /// 
    /// # Safety
    /// This requires kernel support and proper UINTR setup
    pub unsafe fn new(target_id: u64) -> Result<Self> {
        if !is_uintr_supported() {
            return Err(crate::ShmError::InvalidState);
        }
        
        Ok(Self { target_id })
    }

    /// Send a user interrupt
    /// 
    /// # Safety
    /// The target must be set up to receive UINTR
    pub unsafe fn send(&self, _vector: u8) {
        // SENDUIPI instruction would go here
        // This requires inline assembly and kernel support
        // For now, this is a placeholder
    }
}

/// UINTR receiver (can receive interrupts)
pub struct UintrReceiver {
    vector: u8,
    handler: Option<UintrHandler>,
}

impl UintrReceiver {
    /// Create a new UINTR receiver
    /// 
    /// # Safety
    /// This requires kernel support and proper UINTR setup
    pub unsafe fn new(vector: u8) -> Result<Self> {
        if !is_uintr_supported() {
            return Err(crate::ShmError::InvalidState);
        }
        
        Ok(Self {
            vector,
            handler: None,
        })
    }

    /// Register a handler for this UINTR vector
    pub fn register_handler(&mut self, handler: UintrHandler) {
        self.handler = Some(handler);
    }

    /// Enable UINTR reception
    /// 
    /// # Safety
    /// This modifies CPU state
    pub unsafe fn enable(&self) {
        // STUI - Set User Interrupt Flag
        // Requires inline assembly
    }

    /// Disable UINTR reception
    /// 
    /// # Safety
    /// This modifies CPU state
    pub unsafe fn disable(&self) {
        // CLUI - Clear User Interrupt Flag
        // Requires inline assembly
    }

    /// Test UINTR pending
    pub fn test_pending(&self) -> bool {
        // TESTUI instruction
        false
    }
}

/// UINTR-based notification mechanism
/// 
/// This provides the lowest possible latency notification
/// by bypassing the kernel entirely.
pub struct UintrNotification {
    sender: Option<UintrSender>,
    receiver: Option<UintrReceiver>,
}

impl UintrNotification {
    pub fn new() -> Result<Self> {
        if !is_uintr_supported() {
            return Err(crate::ShmError::InvalidState);
        }

        Ok(Self {
            sender: None,
            receiver: None,
        })
    }

    pub fn setup_sender(&mut self, target_id: u64) -> Result<()> {
        unsafe {
            self.sender = Some(UintrSender::new(target_id)?);
        }
        Ok(())
    }

    pub fn setup_receiver(&mut self, vector: u8) -> Result<()> {
        unsafe {
            self.receiver = Some(UintrReceiver::new(vector)?);
        }
        Ok(())
    }

    /// Send notification (ultra-low latency)
    /// 
    /// # Safety
    /// Requires proper kernel UINTR setup
    pub unsafe fn notify(&self, vector: u8) -> Result<()> {
        if let Some(ref sender) = self.sender {
            sender.send(vector);
            Ok(())
        } else {
            Err(crate::ShmError::InvalidState)
        }
    }

    pub fn enable_receiver(&self) -> Result<()> {
        if let Some(ref receiver) = self.receiver {
            unsafe { receiver.enable(); }
            Ok(())
        } else {
            Err(crate::ShmError::InvalidState)
        }
    }
}

/// Fallback to eventfd if UINTR is not available
pub struct UintrFallback {
    inner: crate::eventfd::EventFd,
}

impl UintrFallback {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: crate::eventfd::EventFd::new()?,
        })
    }

    pub fn notify(&self) -> Result<()> {
        self.inner.signal()
    }

    pub fn wait(&self) -> Result<u64> {
        self.inner.wait()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uintr_support() {
        // This will likely return false on most systems
        let supported = is_uintr_supported();
        println!("UINTR supported: {}", supported);
    }
}
