//! Procedural surface operators: parametric primitives, full TRS transforms, welding, curves,
//! attribute transfer, surface scattering, bevelling, QEM decimation, isotropic remeshing,
//! half-space clipping, Voronoi fracture, and LSCM flattening with island packing.
//!
//! Operators are pure functions from immutable `GeoDetail` inputs to a new `GeoDetail`.
//! All of them honour a memory budget and cooperative cancellation through [`SopContext`], and
//! are deterministic: identical inputs produce bit-identical output.

mod bevel;
mod clip;
mod common;
mod curves;
mod earclip;
mod modify;
mod primitives;
mod reduce;
mod remesh;
mod sampling;
mod spatial;
mod trimesh;
mod uv;

pub use bevel::poly_bevel;
pub use clip::{clip_halfspace, voronoi_fracture};
pub use common::{SopContext, signed_volume};
pub use curves::{resample_curves, sweep_curves};
pub use modify::{Trs, fuse_points, measure, reverse_winding, transform_trs, triangulate};
pub use primitives::{create_line, create_sphere, create_torus, create_tube};
pub use reduce::poly_reduce;
pub use remesh::{RemeshParams, remesh};
pub use sampling::{SurfaceScatter, attrib_transfer, scatter_surface};
pub use uv::{uv_flatten, uv_layout};
