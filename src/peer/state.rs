use std::collections::HashSet;

/// Shared download state for all peer workers
#[derive(Debug)]
pub struct DownloadState {
    pub total_pieces: usize,
    pub piece_length: u32,
    pub info_hash: [u8; 20],
    pub pieces: Vec<Option<Vec<u8>>>, // `None` = not downloaded, `Some(data)` = done
    pub requested: HashSet<usize>,    // Which pieces are currently being downloaded
}

impl DownloadState {
    pub fn new(total_pieces: usize, piece_length: u32, info_hash: [u8; 20]) -> Self {
        Self {
            total_pieces,
            piece_length,
            info_hash,
            pieces: vec![None; total_pieces],
            requested: HashSet::new(),
        }
    }

    /// Try to find a missing piece this peer has and we don’t
    pub fn pick_piece(&self, peer_bitfield: &[bool]) -> Option<usize> {
        for (i, piece) in self.pieces.iter().enumerate() {
            if piece.is_none()
                && peer_bitfield.get(i).copied().unwrap_or(false)
                && !self.requested.contains(&i)
            {
                return Some(i);
            }
        }
        None
    }

    /// Mark that we are trying to download a piece
    pub fn mark_requested(&mut self, index: usize) {
        self.requested.insert(index);
    }

    /// Once we got the piece data, store it and stop tracking it
    pub fn complete_piece(&mut self, index: usize, data: Vec<u8>) {
        self.pieces[index] = Some(data);
        self.requested.remove(&index);
    }

    /// Has all pieces?
    pub fn is_complete(&self) -> bool {
        self.pieces.iter().all(|p| p.is_some())
    }

    /// How many pieces downloaded
    pub fn progress(&self) -> usize {
        self.pieces.iter().filter(|p| p.is_some()).count()
    }
}
