use windows::Win32::System::LibraryLoader::GetModuleHandleA;

unsafe fn get_module_base() -> isize {
    GetModuleHandleA(None).expect("get module base").0
}

/// Type for finding the offset of something within a running program
pub struct Offset(usize);

impl Offset {
    /// Create an [`Offset`] that calculates the offset of a programs base address
    pub const fn new(offset: usize) -> Self {
        Self(offset)
    }

    pub unsafe fn get_address(&self) -> usize {
        let base = get_module_base() as usize;
        base + self.0
    }
}