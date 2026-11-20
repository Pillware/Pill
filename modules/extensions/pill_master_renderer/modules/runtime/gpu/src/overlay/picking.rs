//! Reading the pick buffer back without stalling a frame.
//!
//! # Responsibilities
//!
//! - Copy each requested pixel of the pick buffer into a buffer the CPU can
//!   map ([`PickReadback::copy_requested_pixels`]).
//! - Ask for those buffers to be mapped once the frame that filled them is
//!   submitted, then turn what they hold into answers
//!   ([`PickReadback::collect`]).
//!
//! # Design
//!
//! A frame records the copies and never waits for them. The map is asked for
//! after the frame is submitted, and a later [`PickReadback::collect`] polls
//! the device without blocking and decodes whatever has arrived.
//!
//! A pixel that covers a mesh holds that instance's position in the frame's
//! draw queue, so each readback keeps the entity at every queue position of
//! the frame it came from. The answer stays right even when a later frame
//! draws the queue in another order.

// Standard library

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::Entity;
use pill_renderer_api::{decode_pick_value, PickHit, PickRequest, PickResult};

// Current crate
use crate::readback::{MappedReadback, Readback};

// =============================================================================
// Constants
// =============================================================================

/// Bytes from one copied pixel's slot to the next: one wgpu row alignment
/// (`COPY_BYTES_PER_ROW_ALIGNMENT`, 256 bytes).
const BYTES_PER_SLOT: u64 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64;

// =============================================================================
// PendingReadback
// =============================================================================

/// The copies one frame recorded for one surface, kept until their pixels can
/// be read.
struct PendingReadback {
    /// One slot of [`BYTES_PER_SLOT`] bytes per request, in request order.
    readback: MappedReadback,
    /// The requests, one per slot.
    requests: Vec<PickRequest>,
    /// The entity at each queue position of the frame that was drawn.
    queue_entities: Vec<Entity>,
}

// =============================================================================
// PickReadback
// =============================================================================

/// Every pick on its way back from the GPU, and the answers that have already
/// arrived.
#[derive(Default)]
pub(super) struct PickReadback {
    /// Readbacks recorded, not answered yet.
    pending_readbacks: Vec<PendingReadback>,
    /// Answers ready to hand out.
    finished_answers: Vec<PickResult>,
}

impl PickReadback {
    /// Answers `request` straight away, without reading the GPU.
    pub fn answer(&mut self, request: PickRequest, hit: PickHit) {
        self.finished_answers.push(PickResult { request, hit });
    }

    /// Records a copy of each request's pixel out of `pick_texture`.
    ///
    /// A request whose pixel lies outside the texture has nothing to copy, so
    /// it is answered with [`PickHit::Nothing`] here.
    pub fn copy_requested_pixels(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        pick_texture: &wgpu::Texture,
        requests: Vec<PickRequest>,
        queue_entities: Vec<Entity>,
    ) {
        // Step 1: Answer a request outside the texture now, since there is no
        // pixel to copy for it.
        let pick_texture_size = pick_texture.size();
        let (requests_inside, requests_outside): (Vec<PickRequest>, Vec<PickRequest>) =
            requests.into_iter().partition(|request| {
                request.x < pick_texture_size.width && request.y < pick_texture_size.height
            });
        for request in requests_outside {
            self.answer(request, PickHit::Nothing);
        }
        if requests_inside.is_empty() {
            return;
        }
        // Step 2: Make one buffer with a slot for every remaining request.
        let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pick_readback"),
            size: BYTES_PER_SLOT * requests_inside.len() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        // Step 3: Record one copy of each request's single pixel.
        for (slot_index, request) in requests_inside.iter().enumerate() {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: pick_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: request.x,
                        y: request.y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback_buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: BYTES_PER_SLOT * slot_index as u64,
                        bytes_per_row: None,
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        // Step 4: Keep the buffer with its requests and the frame's entities,
        // so their answers can be decoded when the bytes arrive.
        self.pending_readbacks.push(PendingReadback {
            readback: MappedReadback::new(readback_buffer),
            requests: requests_inside,
            queue_entities,
        });
    }

    /// Starts mapping every readback recorded since the last call.
    ///
    /// Call it after the frame that recorded them is submitted: a buffer asked
    /// to map before its copy is submitted makes the submission fail.
    pub fn start_mapping(&mut self) {
        for pending_readback in &mut self.pending_readbacks {
            pending_readback.readback.start();
        }
    }

    /// Returns every answer that has arrived.
    ///
    /// The device is polled without waiting, so a readback still in flight is
    /// answered on a later call. A readback the GPU could not deliver answers
    /// [`PickHit::Nothing`] once for each request it carried.
    pub fn collect(&mut self, device: &wgpu::Device) -> Vec<PickResult> {
        // Step 1: Ask for progress without waiting. A readback that is not
        // done yet is answered on a later call.
        if !self.pending_readbacks.is_empty() {
            let _ = device.poll(wgpu::PollType::Poll);
        }
        // Step 2: Poll every readback. One the GPU has finished is decoded
        // into answers, one that failed answers nothing, and one still running
        // goes back into the pending list.
        let mut still_pending_readbacks = Vec::with_capacity(self.pending_readbacks.len());
        for PendingReadback {
            mut readback,
            requests,
            queue_entities,
        } in self.pending_readbacks.drain(..)
        {
            let outcome = readback.poll(|mapped_bytes| {
                (0..requests.len())
                    .map(|slot_index| decode_slot(mapped_bytes, slot_index, &queue_entities))
                    .collect::<Vec<_>>()
            });
            match outcome {
                Readback::Done(hits) => {
                    for (request, hit) in requests.into_iter().zip(hits) {
                        self.finished_answers.push(PickResult { request, hit });
                    }
                }
                // A failed readback answers every request it carried with
                // nothing rather than leaving its caller waiting forever. The
                // warning is logged once, when the failure is found.
                Readback::Failed => {
                    pill_core::warn!(
                        target: telemetry_target::RENDERING,
                        requests = requests.len(),
                        "[render] pick readback failed; its requests answer nothing"
                    );
                    self.finished_answers
                        .extend(answer_every_request_with_nothing(requests));
                }
                Readback::Pending => still_pending_readbacks.push(PendingReadback {
                    readback,
                    requests,
                    queue_entities,
                }),
            }
        }
        self.pending_readbacks = still_pending_readbacks;
        // Step 3: Hand out every answer that is ready.
        std::mem::take(&mut self.finished_answers)
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Answers every request in `requests` with [`PickHit::Nothing`].
///
/// A readback that never arrived leaves one answer per request, so no caller
/// waits for an answer that cannot come.
fn answer_every_request_with_nothing(requests: Vec<PickRequest>) -> Vec<PickResult> {
    requests
        .into_iter()
        .map(|request| PickResult {
            request,
            hit: PickHit::Nothing,
        })
        .collect()
}

/// Decodes slot `slot_index` of a mapped readback into the pick answer it
/// holds.
fn decode_slot(mapped_bytes: &[u8], slot_index: usize, queue_entities: &[Entity]) -> PickHit {
    let slot_start = slot_index * BYTES_PER_SLOT as usize;
    let pick_value = mapped_bytes
        .get(slot_start..slot_start + 4)
        .map_or(0, |pixel_bytes| {
            u32::from_le_bytes([
                pixel_bytes[0],
                pixel_bytes[1],
                pixel_bytes[2],
                pixel_bytes[3],
            ])
        });
    decode_pick_value(pick_value, queue_entities)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Each slot is read at its own aligned offset.
    #[test]
    fn slots_decode_at_their_offsets() {
        let mut world = pill_engine::World::new();
        let first_entity = world.reserve_entity();
        let second_entity = world.reserve_entity();
        let mut mapped_bytes = vec![0u8; BYTES_PER_SLOT as usize * 3];
        mapped_bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
        mapped_bytes[BYTES_PER_SLOT as usize..BYTES_PER_SLOT as usize + 4]
            .copy_from_slice(&0u32.to_le_bytes());
        let handle = pill_renderer_api::PICK_HANDLE_BIT | 7;
        mapped_bytes[2 * BYTES_PER_SLOT as usize..2 * BYTES_PER_SLOT as usize + 4]
            .copy_from_slice(&handle.to_le_bytes());
        let queue_entities = [first_entity, second_entity];
        assert_eq!(
            decode_slot(&mapped_bytes, 0, &queue_entities),
            PickHit::Entity(second_entity)
        );
        assert_eq!(
            decode_slot(&mapped_bytes, 1, &queue_entities),
            PickHit::Nothing
        );
        assert_eq!(
            decode_slot(&mapped_bytes, 2, &queue_entities),
            PickHit::Handle(7)
        );
    }

    /// An answer given straight away is already waiting in `finished_answers`.
    #[test]
    fn immediate_answers_are_collected() {
        let mut readback = PickReadback::default();
        let request = PickRequest {
            id: 3,
            surface: pill_renderer_api::SurfaceId::PRIMARY,
            x: 1,
            y: 1,
        };
        readback.answer(request, PickHit::Nothing);
        assert_eq!(readback.finished_answers.len(), 1);
        assert_eq!(
            std::mem::take(&mut readback.finished_answers)[0].request.id,
            3
        );
    }

    /// Every request a failed readback carried gets one `Nothing` answer, so no
    /// caller waits for an answer that cannot come.
    #[test]
    fn a_failed_readback_answers_every_request_with_nothing() {
        let requests: Vec<PickRequest> = [11, 12]
            .into_iter()
            .map(|id| PickRequest {
                id,
                surface: pill_renderer_api::SurfaceId::PRIMARY,
                x: 4,
                y: 4,
            })
            .collect();

        let answers = answer_every_request_with_nothing(requests);

        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0].request.id, 11);
        assert_eq!(answers[1].request.id, 12);
        assert!(answers.iter().all(|answer| answer.hit == PickHit::Nothing));
    }
}
