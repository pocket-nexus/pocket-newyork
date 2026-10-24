//! The cells' records of `NEAR`, read from the pack while the eye moves.
//!
//! One block of GPU-mapped memory is cut into slots of the largest record's size; which record goes to which slot
//! is `ny_frame::near::Slots`. A thread of its own reads the records into their slots. A slot is written again only
//! when the GPU has done with the frames that drew from it (`HELD` frames later).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::mpsc::{channel, Receiver, Sender};

use ny_frame::near::Slots;
use ny_pack::NearCell;
use pocket_vita_gxm::mem::{Block, Kind};

/// Frames a slot stays untouched after its last draw before another record may go into it: the GPU may still be
/// drawing the two frames before.
const HELD: u32 = 3;

/// A read for the thread: the record's place in the file, and where it goes.
struct Read_ {
    cell: u32,
    slot: usize,
    offset: u64,
    size: usize,
    to: usize,
}

pub struct Near {
    _block: Block,
    base: *mut u8,
    slot_bytes: usize,
    pub slots: Slots,
    to_thread: Sender<Read_>,
    from_thread: Receiver<(u32, usize, bool)>,
}

unsafe impl Send for Read_ {}

impl Near {
    /// # Safety
    /// GXM is up.
    pub unsafe fn new(path: &str, section: u64, slot_bytes: usize, slots: usize) -> Result<Near, String> {
        let slot_bytes = (slot_bytes + 4095) & !4095;
        let mut block = Block::with_access(Kind::Main, slot_bytes * slots, false)?;
        let base = block.alloc(slot_bytes * slots, 4096).ok_or("near slots")?;
        let (to_thread, rx) = channel::<Read_>();
        let (tx, from_thread) = channel::<(u32, usize, bool)>();
        let mut file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
        std::thread::Builder::new()
            .name("newyork-near".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                while let Ok(r) = rx.recv() {
                    let ok = file.seek(SeekFrom::Start(section + r.offset)).is_ok() && {
                        let dst = unsafe { core::slice::from_raw_parts_mut(r.to as *mut u8, r.size) };
                        file.read_exact(dst).is_ok()
                    };
                    if tx.send((r.cell, r.slot, ok)).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Near { _block: block, base, slot_bytes, slots: Slots::new(slots, HELD), to_thread, from_thread })
    }

    /// Takes in what the thread has read.
    pub fn receive(&mut self) {
        while let Ok((cell, slot, ok)) = self.from_thread.try_recv() {
            self.slots.arrived(cell, slot, ok);
        }
    }

    /// Where a cell's record is, when it has been read.
    pub fn ready(&self, cell: u32) -> Option<*const u8> {
        self.slots.ready(cell).map(|slot| unsafe { self.base.add(slot * self.slot_bytes) as *const u8 })
    }

    /// Notes that this frame draws from a cell's record.
    pub fn touch(&mut self, cell: u32, frame: u32) {
        self.slots.touch(cell, frame);
    }

    /// Asks for the records of `wanted` cells (with their distances), nearest first.
    pub fn request(&mut self, wanted: &mut Vec<(u32, f32)>, cells: &[NearCell], frame: u32) {
        let (base, slot_bytes, to_thread) = (self.base as usize, self.slot_bytes, &self.to_thread);
        self.slots.request(wanted, cells, frame, |r| {
            let _ = to_thread.send(Read_ { cell: r.cell, slot: r.slot, offset: r.offset, size: r.size, to: base + r.slot * slot_bytes });
        });
    }

    /// Slots holding a record, of all.
    pub fn filled(&self) -> (usize, usize) {
        self.slots.filled()
    }
}
