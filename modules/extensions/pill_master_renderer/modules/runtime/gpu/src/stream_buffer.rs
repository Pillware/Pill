//! A GPU buffer rewritten for each surface, kept between frames.
//!
//! # Responsibilities
//!
//! - [`StreamBuffer`]: uploads one surface's bytes into one GPU buffer, and
//!   replaces that buffer with a larger one when the bytes do not fit.
//!
//! # Design
//!
//! The overlay's geometry and the UI painter's lists are rebuilt on the
//! processor for every surface, so neither can be uploaded once and reused as
//! it is. A [`StreamBuffer`] keeps one GPU buffer and rewrites it for each
//! surface, so a frame allocates only when a surface's bytes outgrow it.
//!
//! A queue write is applied at the next submission, and each surface submits
//! its own frame. Writing the buffer once per surface is therefore enough for
//! every surface of a frame. A caller must never write the same buffer twice
//! before one submission, because the second write replaces the first.

// =============================================================================
// Stream Buffer
// =============================================================================

/// A buffer that is rewritten for each surface and replaced by a larger one
/// when the bytes do not fit.
pub(crate) struct StreamBuffer {
    debug_label: &'static str,
    usage: wgpu::BufferUsages,
    allocated_buffer: Option<wgpu::Buffer>,
    capacity_bytes: u64,
}

impl StreamBuffer {
    /// Creates an empty stream for buffers of `usage` (`VERTEX`, `INDEX`, ...).
    /// The first write allocates the GPU buffer, and adds `COPY_DST` to `usage`
    /// because every write copies bytes into it.
    pub(crate) fn new(debug_label: &'static str, usage: wgpu::BufferUsages) -> Self {
        Self {
            debug_label,
            usage,
            allocated_buffer: None,
            capacity_bytes: 0,
        }
    }

    /// Uploads `bytes` into the buffer and returns it, or returns `None` when
    /// `bytes` is empty.
    ///
    /// Empty input allocates nothing and writes nothing, so the caller can skip
    /// its draw for that surface.
    pub(crate) fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bytes: &[u8],
    ) -> Option<&wgpu::Buffer> {
        if bytes.is_empty() {
            return None;
        }
        // Step 1: Replace the buffer when this surface's bytes do not fit.
        let needed_bytes = bytes.len() as u64;
        if self.allocated_buffer.is_none() || needed_bytes > self.capacity_bytes {
            self.capacity_bytes = needed_bytes.next_power_of_two().max(4096);
            self.allocated_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.debug_label),
                size: self.capacity_bytes,
                usage: self.usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        // Step 2: Copy the bytes in; the draw recorded after this reads them.
        let written_buffer = self.allocated_buffer.as_ref()?;
        queue.write_buffer(written_buffer, 0, bytes);
        Some(written_buffer)
    }
}
