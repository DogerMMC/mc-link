use std::collections::{HashMap, BinaryHeap};
use std::cmp::Ordering;

#[derive(Debug, Clone)]
pub struct RelayNode {
    pub id: String,
    pub address: String,
}

#[derive(Debug, Clone)]
pub struct RelayTopology {
    pub relays: Vec<RelayNode>,
    pub latency_matrix: HashMap<String, HashMap<String, u64>>,
}

#[derive(Clone)]
struct DijkstraState {
    node_id: String,
    cost: u64,
    path: Vec<String>,
}

impl PartialEq for DijkstraState {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}

impl Eq for DijkstraState {}

impl PartialOrd for DijkstraState {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cost.cmp(&other.cost).reverse())
    }
}

impl Ord for DijkstraState {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cost.cmp(&other.cost).reverse()
    }
}

impl RelayTopology {
    pub fn find_best_path(&self, from_id: &str, to_id: &str) -> Option<(Vec<String>, u64)> {
        if from_id == to_id {
            return Some((vec![from_id.to_string()], 0));
        }

        let mut heap = BinaryHeap::new();
        let mut dist: HashMap<String, u64> = HashMap::new();
        let mut prev: HashMap<String, String> = HashMap::new();

        dist.insert(from_id.to_string(), 0);
        heap.push(DijkstraState {
            node_id: from_id.to_string(),
            cost: 0,
            path: vec![from_id.to_string()],
        });

        while let Some(state) = heap.pop() {
            let node = &state.node_id;

            if state.cost > *dist.get(node).unwrap_or(&u64::MAX) {
                continue;
            }

            if node == to_id {
                let path = state.path;
                let total_cost = dist.get(to_id).unwrap_or(&u64::MAX);
                return Some((path, *total_cost));
            }

            if let Some(neighbors) = self.latency_matrix.get(node) {
                for (neighbor, &latency) in neighbors {
                    let new_cost = state.cost.saturating_add(latency);

                    if new_cost < *dist.get(neighbor).unwrap_or(&u64::MAX) {
                        dist.insert(neighbor.clone(), new_cost);
                        prev.insert(neighbor.clone(), node.clone());

                        let mut new_path = state.path.clone();
                        new_path.push(neighbor.clone());

                        heap.push(DijkstraState {
                            node_id: neighbor.clone(),
                            cost: new_cost,
                            path: new_path,
                        });
                    }
                }
            }
        }

        None
    }

    pub fn get_relay_address(&self, relay_id: &str) -> Option<String> {
        self.relays.iter().find(|r| r.id == relay_id).map(|r| r.address.clone())
    }
}

pub fn select_optimal_relay_chain(
    topology: &RelayTopology,
    host_relay_id: &str,
    client_relay_id: &str,
) -> Option<(Vec<String>, u64)> {
    topology.find_best_path(host_relay_id, client_relay_id)
}
