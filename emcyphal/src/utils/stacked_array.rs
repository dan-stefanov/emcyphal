/// Array of N1 + N2 elements with uniform access
///
/// This is a workaround for [T; {N1 + N2}] in stable rust
#[repr(C)]
pub struct DuplexArray<T, const N1: usize, const N2: usize>([T; N1], [T; N2]);

impl<T, const N1: usize, const N2: usize> DuplexArray<T, N1, N2> {
    #[allow(clippy::redundant_closure)]
    pub fn from_fn(mut cb: impl FnMut(usize) -> T) -> Self {
        Self(
            core::array::from_fn(|i| cb(i)),
            core::array::from_fn(|i| cb(N1 + i)),
        )
    }
}

impl<T: Clone, const N1: usize, const N2: usize> DuplexArray<T, N1, N2> {
    #[allow(clippy::redundant_closure)]
    pub fn repeat(val: T) -> Self {
        Self(core::array::repeat(val.clone()), core::array::repeat(val))
    }
}

impl<T: Default, const N1: usize, const N2: usize> Default for DuplexArray<T, N1, N2> {
    fn default() -> Self {
        Self(
            core::array::from_fn(|_| Default::default()),
            core::array::from_fn(|_| Default::default()),
        )
    }
}

impl<T, const N1: usize, const N2: usize> core::ops::Deref for DuplexArray<T, N1, N2> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        // SAFETY: DuplexArray is #[repr(C)] so the two arrays are laid out contiguously
        unsafe { core::slice::from_raw_parts(self.0.as_ptr(), N1 + N2) }
    }
}

impl<T, const N1: usize, const N2: usize> core::ops::DerefMut for DuplexArray<T, N1, N2> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: DuplexArray is #[repr(C)] so the two arrays are laid out contiguously
        unsafe { core::slice::from_raw_parts_mut(self.0.as_mut_ptr(), N1 + N2) }
    }
}

/// Array of N1 + N2 + N3 elements with uniform access
///
/// This is a workaround for [T; {N1 + N2 + N3}] in stable rust
#[repr(C)]
pub struct TriplexArray<T, const N1: usize, const N2: usize, const N3: usize>(
    [T; N1],
    [T; N2],
    [T; N3],
);

impl<T, const N1: usize, const N2: usize, const N3: usize> TriplexArray<T, N1, N2, N3> {
    #[allow(clippy::redundant_closure)]
    pub fn from_fn(mut cb: impl FnMut(usize) -> T) -> Self {
        Self(
            core::array::from_fn(|i| cb(i)),
            core::array::from_fn(|i| cb(N1 + i)),
            core::array::from_fn(|i| cb(N1 + N2 + i)),
        )
    }

    pub const fn len(&self) -> usize {
        N1 + N2 + N3
    }
}

impl<T: Default, const N1: usize, const N2: usize, const N3: usize> Default
    for TriplexArray<T, N1, N2, N3>
{
    fn default() -> Self {
        Self(
            core::array::from_fn(|_| Default::default()),
            core::array::from_fn(|_| Default::default()),
            core::array::from_fn(|_| Default::default()),
        )
    }
}

impl<T, const N1: usize, const N2: usize, const N3: usize> core::ops::Deref
    for TriplexArray<T, N1, N2, N3>
{
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        // SAFETY: TriplexArray is #[repr(C)] so the three arrays are laid out contiguously
        unsafe { core::slice::from_raw_parts(self.0.as_ptr(), N1 + N2 + N3) }
    }
}

impl<T, const N1: usize, const N2: usize, const N3: usize> core::ops::DerefMut
    for TriplexArray<T, N1, N2, N3>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: TriplexArray is #[repr(C)] so the three arrays are laid out contiguously
        unsafe { core::slice::from_raw_parts_mut(self.0.as_mut_ptr(), N1 + N2 + N3) }
    }
}
