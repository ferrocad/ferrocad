//! Connected components of a mesh.
//!
//! Components are taken over shared **points**, matching upstream
//! `MeshObject::getSeparateComponents`: two facets belong to the same component
//! when they address any common point, even if they do not share an edge.
//!
//! [`Mesh::separate_components`] compacts each component's points in first-seen
//! order, preserves the facet order, and copies the placement. Keeping the
//! placement (rather than baking it) leaves every returned mesh in the same
//! local frame as the source.

use std::collections::HashMap;

use ferrocad_types::Placement;

use crate::Mesh;

impl Mesh {
    /// The number of connected components over shared points
    /// (upstream `Mesh.countComponents()`).
    ///
    /// Points that no facet uses are not components: an empty facet list has
    /// zero components.
    pub fn count_components(&self) -> usize {
        components(self).len()
    }

    /// One mesh per connected component over shared points
    /// (upstream `getSeparateComponents()`).
    ///
    /// Every returned mesh keeps this mesh's placement; facets and the points
    /// they use keep their relative order. An empty mesh returns an empty list.
    pub fn separate_components(&self) -> Vec<Mesh> {
        let placement = self.placement();
        components(self)
            .into_iter()
            .map(|facets| self.compact_component(&facets, placement))
            .collect()
    }

    /// Build one component's mesh, renumbering its points in first-seen order.
    fn compact_component(&self, facets: &[usize], placement: Placement) -> Mesh {
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let mut points = Vec::new();
        let mut indices = Vec::with_capacity(facets.len());
        for &facet_index in facets {
            let facet = self.facets()[facet_index];
            let mut mapped = [0u32; 3];
            for (slot, &point_index) in facet.iter().enumerate() {
                mapped[slot] = *remap.entry(point_index).or_insert_with(|| {
                    points.push(self.points()[point_index as usize]);
                    (points.len() - 1) as u32
                });
            }
            indices.push(mapped);
        }
        let mut mesh =
            Mesh::new(points, indices).expect("a subset of a valid mesh's facets is still valid");
        mesh.set_placement(placement);
        mesh
    }
}

/// Group facet indices by the point-component their vertices belong to.
///
/// The groups, and the facets inside each group, follow first-seen order.
fn components(mesh: &Mesh) -> Vec<Vec<usize>> {
    let facet_count = mesh.count_facets();
    if facet_count == 0 {
        return Vec::new();
    }

    let mut sets = DisjointSet::new(mesh.count_points());
    for facet in mesh.facets() {
        sets.union(facet[0] as usize, facet[1] as usize);
        sets.union(facet[1] as usize, facet[2] as usize);
    }

    let mut slot_of_root: HashMap<usize, usize> = HashMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, facet) in mesh.facets().iter().enumerate() {
        let root = sets.find(facet[0] as usize);
        let slot = *slot_of_root.entry(root).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[slot].push(index);
    }
    groups
}

/// Union-find with path halving and union by rank.
struct DisjointSet {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl DisjointSet {
    fn new(size: usize) -> Self {
        DisjointSet {
            parent: (0..size).collect(),
            rank: vec![0; size],
        }
    }

    fn find(&mut self, mut index: usize) -> usize {
        while self.parent[index] != index {
            self.parent[index] = self.parent[self.parent[index]];
            index = self.parent[index];
        }
        index
    }

    fn union(&mut self, first: usize, second: usize) {
        let mut first = self.find(first);
        let mut second = self.find(second);
        if first == second {
            return;
        }
        if self.rank[first] < self.rank[second] {
            std::mem::swap(&mut first, &mut second);
        }
        self.parent[second] = first;
        if self.rank[first] == self.rank[second] {
            self.rank[first] += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_types::{Placement, Rotation, Vector3};

    fn translated_box(offset: Vector3) -> Mesh {
        let mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let points = mesh
            .points()
            .iter()
            .map(|point| point.add(&offset))
            .collect();
        Mesh::new(points, mesh.facets().to_vec()).unwrap()
    }

    fn two_box_mesh(first: Mesh, second: Mesh) -> Mesh {
        let mut points = first.points().to_vec();
        let base = points.len() as u32;
        points.extend_from_slice(second.points());
        let facets = first
            .facets()
            .iter()
            .copied()
            .chain(
                second
                    .facets()
                    .iter()
                    .map(|facet| [facet[0] + base, facet[1] + base, facet[2] + base]),
            )
            .collect();
        Mesh::new(points, facets).unwrap()
    }

    #[test]
    fn two_apart_boxes_are_two_components() {
        let first = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let second = translated_box(Vector3::new(10.0, 0.0, 0.0));
        let mut mesh = two_box_mesh(first, second);
        let placement = Placement::new(Vector3::new(1.0, 2.0, 3.0), Rotation::identity());
        mesh.set_placement(placement);

        assert_eq!(mesh.count_components(), 2);
        let components = mesh.separate_components();
        assert_eq!(components.len(), 2);
        for component in &components {
            assert!(component.is_solid());
            assert_eq!(component.volume(), 1.0);
            assert_eq!(component.placement(), placement);
        }
        assert_eq!(mesh.volume(), 2.0);
    }

    #[test]
    fn a_single_box_is_one_component_and_an_empty_mesh_has_none() {
        assert_eq!(Mesh::box_(1.0, 1.0, 1.0).unwrap().count_components(), 1);
        assert_eq!(Mesh::empty().count_components(), 0);
        assert!(Mesh::empty().separate_components().is_empty());
    }

    #[test]
    fn facets_sharing_only_a_point_are_one_component() {
        // Two triangles that share only one point are a single component over
        // points, even though they share no edge.
        let mesh = Mesh::new(
            vec![
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(-1.0, 0.0, 0.0),
                Vector3::new(0.0, -1.0, 0.0),
            ],
            vec![[0, 1, 2], [0, 3, 4]],
        )
        .unwrap();
        assert_eq!(mesh.count_components(), 1);
        assert_eq!(mesh.separate_components()[0].count_facets(), 2);
    }
}
