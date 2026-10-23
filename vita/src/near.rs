//! The cells' records of `NEAR`, read from the pack while the eye moves.
//!
//! One block of GPU-mapped memory is cut into slots of the largest record's size. A frame asks for the cells within
//! the near reach whose records are not in a slot; a thread of its own reads them, nearest first, into free slots,
//! or into the slot of a cell no frame has drawn for a while. A slot is written again only when the GPU has done
//! with the frames that drew from it (`HELD` frames later).

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::mpsc::{channel, Receiver, Sender};

use ny_pack::NearCell;
use pocket_vita_gxm::mem::{Block, Kind};

/// Frames a slot stays untouched after its last draw before another record may go into it: the GPU may still be
/// drawing the two frames before.
const HELD: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    Reading(u32),
    Ready(u32),
}

struct Slot {
    at: *mut u8,
    state: State,
    used: u32,
}

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
    slots: Vec<Slot>,
    of_cell: HashMap<u32, usize>,
    to_thread: Sender<Read_>,
    from_thread: Receiver<(u32, usize, bool)>,
    /// Records read, and the bytes.
    pub reads: u32,
    pub bytes: u64,
    /// Cells asked for that found no slot.
    pub starved: u32,
}

unsafe impl Send for Read_ {}

impl Near {
    /// # Safety
    /// GXM is up.
    pub unsafe fn new(path: &str, section: u64, slot_bytes: usize, slots: usize) -> Result<Near, String> {
        let slot_bytes = (slot_bytes + 4095) & !4095;
        let mut block = Block::with_access(Kind::Main, slot_bytes * slots, false)?;
        let base = block.alloc(slot_bytes * slots, 4096).ok_or("near slots")?;
        let slots: Vec<Slot> = (0..slots).map(|i| Slot { at: base.add(i * slot_bytes), state: State::Free, used: 0 }).collect();
        let (to_thread, rx) = channel::<Read_>();
        let (tx, from_thread) = channel::<(u32, usize, bool)>();
        let mut file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let base_addr = base as usize;
        std::thread::Builder::new()
            .name("newyork-near".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let _ = base_addr;
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
        Ok(Near { _block: block, slots, of_cell: HashMap::new(), to_thread, from_thread, reads: 0, bytes: 0, starved: 0 })
    }

    /// Takes in what the thread has read.
    pub fn receive(&mut self) {
        while let Ok((cell, slot, ok)) = self.from_thread.try_recv() {
            let s = &mut self.slots[slot];
            if s.state == State::Reading(cell) {
                if ok {
                    s.state = State::Ready(cell);
                } else {
                    s.state = State::Free;
                    self.of_cell.remove(&cell);
                }
            }
        }
    }

    /// Where a cell's record is, when it has been read.
    pub fn ready(&self, cell: u32) -> Option<*const u8> {
        let &slot = self.of_cell.get(&cell)?;
        let s = &self.slots[slot];
        (s.state == State::Ready(cell)).then_some(s.at as *const u8)
    }

    /// Notes that this frame draws from a cell's record.
    pub fn touch(&mut self, cell: u32, frame: u32) {
        if let Some(&slot) = self.of_cell.get(&cell) {
            self.slots[slot].used = frame;
        }
    }

    /// Asks for the records of `wanted` cells (with their distances), nearest first.
    pub fn request(&mut self, wanted: &mut Vec<(u32, f32)>, cells: &[NearCell], frame: u32) {
        wanted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(core::cmp::Ordering::Equal));
        for &(cell, _) in wanted.iter() {
            if self.of_cell.contains_key(&cell) {
                continue;
            }
            let rec = cells[cell as usize];
            if rec.size == 0 {
                continue;
            }
            // A free slot, or the one least lately drawn from that the GPU is done with.
            let slot = match self.slots.iter().position(|s| s.state == State::Free) {
                Some(i) => Some(i),
                None => self
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| matches!(s.state, State::Ready(_)) && s.used + HELD < frame)
                    .min_by_key(|(_, s)| s.used)
                    .map(|(i, _)| i),
            };
            let Some(slot) = slot else {
                self.starved += 1;
                return;
            };
            if let State::Ready(old) = self.slots[slot].state {
                self.of_cell.remove(&old);
            }
            let s = &mut self.slots[slot];
            s.state = State::Reading(cell);
            s.used = frame;
            self.of_cell.insert(cell, slot);
            self.reads += 1;
            self.bytes += rec.size as u64;
            let _ = self.to_thread.send(Read_ { cell, slot, offset: rec.offset as u64, size: rec.size as usize, to: s.at as usize });
        }
    }

    /// Slots holding a record, of all.
    pub fn filled(&self) -> (usize, usize) {
        (self.slots.iter().filter(|s| s.state != State::Free).count(), self.slots.len())
    }
}
