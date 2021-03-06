//! Contains all structures and methods to create and manage mesh scene graph nodes.
//!
//! Mesh is a 3D model, each mesh split into multiple surfaces, each surface holds single
//! part of 3D model that have same textures assigned to each face. Such separation allows
//! us to efficiently render geometry, thus reducing amount of draw calls.
//!
//! Usually there is no need to manually create meshes, it is much easier to make one in 3d
//! modelling software or just download some model you like and load it in engine. But since
//! 3d model can contain multiple nodes, 3d model loading discussed in model resource section.

use crate::core::math::vec3::Vec3;
use crate::scene::node::Node;
use crate::{
    core::{
        color::Color,
        math::{aabb::AxisAlignedBoundingBox, frustum::Frustum},
        visitor::{Visit, VisitResult, Visitor},
    },
    renderer::surface::Surface,
    scene::{base::Base, base::BaseBuilder, graph::Graph},
};
use rg3d_core::math::mat4::Mat4;
use std::{
    cell::Cell,
    ops::{Deref, DerefMut},
};

/// See module docs.
#[derive(Clone, Debug)]
pub struct Mesh {
    base: Base,
    surfaces: Vec<Surface>,
    bounding_box: Cell<AxisAlignedBoundingBox>,
    bounding_box_dirty: Cell<bool>,
}

impl Default for Mesh {
    fn default() -> Self {
        Self {
            base: Default::default(),
            surfaces: Default::default(),
            bounding_box: Default::default(),
            bounding_box_dirty: Cell::new(true),
        }
    }
}

impl Deref for Mesh {
    type Target = Base;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl DerefMut for Mesh {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

impl Visit for Mesh {
    fn visit(&mut self, name: &str, visitor: &mut Visitor) -> VisitResult {
        visitor.enter_region(name)?;

        self.base.visit("Common", visitor)?;

        // Serialize surfaces, but keep in mind that surfaces from resources will be automatically
        // recreated on resolve stage! Serialization of surfaces needed for procedural surfaces.
        self.surfaces.visit("Surfaces", visitor)?;

        visitor.leave_region()
    }
}

impl Mesh {
    /// Returns shared reference to array of surfaces.
    #[inline]
    pub fn surfaces(&self) -> &[Surface] {
        &self.surfaces
    }

    /// Returns mutable reference to array of surfaces.
    #[inline]
    pub fn surfaces_mut(&mut self) -> &mut [Surface] {
        &mut self.surfaces
    }

    /// Removes all surfaces from mesh.
    #[inline]
    pub fn clear_surfaces(&mut self) {
        self.surfaces.clear();
        self.bounding_box_dirty.set(true);
    }

    /// Adds new surface into mesh, can be used to procedurally generate meshes.
    #[inline]
    pub fn add_surface(&mut self, surface: Surface) {
        self.surfaces.push(surface);
        self.bounding_box_dirty.set(true);
    }

    /// Applies given color to all surfaces.
    #[inline]
    pub fn set_color(&mut self, color: Color) {
        for surface in self.surfaces.iter_mut() {
            surface.set_color(color);
        }
    }

    /// Performs lazy bounding box evaluation. Bounding box presented in *local coordinates*
    /// WARNING: This method does *not* includes bounds of bones!
    pub fn bounding_box(&self) -> AxisAlignedBoundingBox {
        if self.bounding_box_dirty.get() {
            let mut bounding_box = AxisAlignedBoundingBox::default();
            for surface in self.surfaces.iter() {
                let data = surface.data();
                let data = data.lock().unwrap();
                for vertex in data.get_vertices() {
                    bounding_box.add_point(vertex.position);
                }
            }
            self.bounding_box.set(bounding_box);
            self.bounding_box_dirty.set(false);
        }
        self.bounding_box.get()
    }

    /// Calculate bounding box in *world coordinates*. This method is very heavy and not
    /// intended to use every frame! WARNING: This method does *not* includes bounds of bones!
    pub fn world_bounding_box(&self) -> AxisAlignedBoundingBox {
        let mut bounding_box = AxisAlignedBoundingBox::default();
        for surface in self.surfaces.iter() {
            let data = surface.data();
            let data = data.lock().unwrap();
            for vertex in data.get_vertices() {
                bounding_box.add_point(self.global_transform().transform_vector(vertex.position));
            }
        }
        bounding_box
    }

    /// Calculate bounding box in *world coordinates* including influence of bones. This method
    /// is very heavy and not intended to use every frame!
    pub fn full_world_bounding_box(&self, graph: &Graph) -> AxisAlignedBoundingBox {
        let mut bounding_box = AxisAlignedBoundingBox::default();
        for surface in self.surfaces.iter() {
            let data = surface.data();
            let data = data.lock().unwrap();
            if surface.bones().is_empty() {
                for vertex in data.get_vertices() {
                    bounding_box
                        .add_point(self.global_transform().transform_vector(vertex.position));
                }
            } else {
                // Special case for skinned surface. Its actual bounds defined only by bones
                // influence.

                // Precalculate bone matrices first to speed up calculations.
                let bone_matrices = surface
                    .bones()
                    .iter()
                    .map(|&b| {
                        let bone_node = &graph[b];
                        bone_node.global_transform() * bone_node.inv_bind_pose_transform()
                    })
                    .collect::<Vec<Mat4>>();

                for vertex in data.get_vertices() {
                    let mut position = Vec3::ZERO;
                    for (&bone_index, &weight) in
                        vertex.bone_indices.iter().zip(vertex.bone_weights.iter())
                    {
                        position += bone_matrices[bone_index as usize]
                            .transform_vector(vertex.position)
                            .scale(weight);
                    }

                    bounding_box.add_point(position);
                }
            }
        }
        bounding_box
    }

    /// Performs frustum visibility test. It uses mesh bounding box *and* positions of bones.
    /// Mesh is considered visible if its bounding box visible by frustum, or if any bones
    /// position is inside frustum.
    pub fn is_intersect_frustum(&self, graph: &Graph, frustum: &Frustum) -> bool {
        if frustum.is_intersects_aabb_transform(&self.bounding_box(), &self.global_transform) {
            return true;
        }

        for surface in self.surfaces.iter() {
            for &bone in surface.bones.iter() {
                if frustum.is_contains_point(graph[bone].global_position()) {
                    return true;
                }
            }
        }

        false
    }
}

/// Mesh builder allows you to construct mesh in declarative manner.
pub struct MeshBuilder {
    base_builder: BaseBuilder,
    surfaces: Vec<Surface>,
}

impl MeshBuilder {
    /// Creates new instance of mesh builder.
    pub fn new(base_builder: BaseBuilder) -> Self {
        Self {
            base_builder,
            surfaces: Default::default(),
        }
    }

    /// Sets desired surfaces for mesh.
    pub fn with_surfaces(mut self, surfaces: Vec<Surface>) -> Self {
        self.surfaces = surfaces;
        self
    }

    /// Creates new mesh.
    pub fn build(self) -> Mesh {
        Mesh {
            base: self.base_builder.build(),
            surfaces: self.surfaces,
            bounding_box: Default::default(),
            bounding_box_dirty: Cell::new(true),
        }
    }

    /// Creates new node instance.
    pub fn build_node(self) -> Node {
        Node::Mesh(self.build())
    }
}
