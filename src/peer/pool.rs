use std::net::SocketAddr;
use rustc_hash::FxHashMap;

use super::connection::PeerConnection;

/// Indexed connections for more efficient peer lookups
#[derive(Debug)]
pub struct IndexedConnections {
    connections: Vec<Option<PeerConnection>>,
    addr_to_index: FxHashMap<SocketAddr, usize>,
    free_indices: Vec<usize>,
}

impl Default for IndexedConnections {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexedConnections {
    pub fn new() -> Self {
        Self {
            connections: Vec::new(),
            addr_to_index: FxHashMap::default(),
            free_indices: Vec::new(),
        }
    }

    pub fn insert(&mut self, addr: SocketAddr, conn: PeerConnection) {
        if let Some(index) = self.free_indices.pop() {
            self.connections[index] = Some(conn);
            self.addr_to_index.insert(addr, index);
        } else {
            let index = self.connections.len();
            self.connections.push(Some(conn));
            self.addr_to_index.insert(addr, index);
        }
    }

    pub fn get(&self, addr: &SocketAddr) -> Option<&PeerConnection> {
        self.addr_to_index
            .get(addr)
            .and_then(|&idx| self.connections.get(idx))
            .and_then(|opt| opt.as_ref())
    }

    pub fn get_mut(&mut self, addr: &SocketAddr) -> Option<&mut PeerConnection> {
        self.addr_to_index
            .get(addr)
            .and_then(|&idx| self.connections.get_mut(idx))
            .and_then(|opt| opt.as_mut())
    }

    pub fn remove(&mut self, addr: &SocketAddr) -> Option<PeerConnection> {
        if let Some(&index) = self.addr_to_index.get(addr) {
            self.addr_to_index.remove(addr);
            let removed = self.connections[index].take();
            self.free_indices.push(index);
            removed
        } else {
            None
        }
    }

    pub fn contains_key(&self, addr: &SocketAddr) -> bool {
        self.addr_to_index.contains_key(addr)
    }

    pub fn is_empty(&self) -> bool {
        self.addr_to_index.is_empty()
    }

    pub fn len(&self) -> usize {
        self.addr_to_index.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SocketAddr, &PeerConnection)> {
        self.addr_to_index
            .iter()
            .filter_map(move |(addr, &idx)| self.connections[idx].as_ref().map(|conn| (addr, conn)))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&SocketAddr, &mut PeerConnection)> {
        // borrow split: addr_to_index is read-only while connections is mutably iterated
        let addr_to_index = &self.addr_to_index;
        self.connections
            .iter_mut()
            .enumerate()
            .filter_map(|(idx, conn_opt)| {
                if let Some(conn) = conn_opt.as_mut() {
                    addr_to_index
                        .iter()
                        .find(|&(_, &i)| i == idx)
                        .map(|(addr, _)| (addr, conn))
                } else {
                    None
                }
            })
    }
}
