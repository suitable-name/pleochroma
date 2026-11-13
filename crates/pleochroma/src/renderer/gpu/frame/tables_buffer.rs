//! The GPU table buffer of the optics lookup tables, and the renderer's use of it.
//!
//! A class with [`material_class::OPTICS_TABLES`] compiles the table variant of the megakernel
//! (`super::tables_source`) and binds one storage buffer, built from the material's index range
//! by `optics::tables::pack_gpu_words`, at `OPTICS_TABLES_BINDING` of every bind group of that
//! pipeline. [`OpticsTablesGpu`] holds the buffer and rebuilds it only when the encoded material
//! changes, so a progressive render builds the tables once.
//!
//! A class without the bit never reaches this buffer: its sources, bind groups and uploads
//! are the plain ones.

use std::sync::Once;

use super::{
    GpuPipelineKind,
    renderer::GpuFrameRenderer,
    specialisation::material_class,
    tables_source::{megakernel_tables_source, megakernel_uses_tables},
};
use crate::{
    optics::{
        materials::GemMaterial,
        tables::{OpticsTableSet, pack_disabled_gpu_words, pack_gpu_words, upload_gpu_buffer},
    },
    renderer::{buffers::GpuGemMaterial, gpu::MEGAKERNEL_STORAGE_BUFFERS},
};

/// Cache key of the disabled buffer; its length differs from every encoded material's.
const DISABLED_KEY: &[u8] = b"disabled";

/// The optics-table buffer of one render, rebuilt only when the encoded material changes.
#[derive(Default)]
pub struct OpticsTablesGpu {
    /// The bytes of the encoded material the buffer was built for (or [`DISABLED_KEY`]).
    key: Vec<u8>,
    buffer: Option<wgpu::Buffer>,
}

impl OpticsTablesGpu {
    /// Makes the buffer hold the tables of `material`, building them unless `key` (the bytes of
    /// the encoded material) is the one the buffer was built for.
    pub fn ensure(&mut self, device: &wgpu::Device, material: &GemMaterial, key: &[u8]) {
        if self.buffer.is_some() && self.key == key {
            return;
        }
        let set = OpticsTableSet::for_material(material);
        self.buffer = Some(upload_gpu_buffer(device, &pack_gpu_words(&set)));
        self.key.clear();
        self.key.extend_from_slice(key);
    }

    /// Makes the buffer a disabled one: a table kernel then runs the shipped closed forms.
    pub fn ensure_disabled(&mut self, device: &wgpu::Device) {
        if self.buffer.is_some() && self.key == DISABLED_KEY {
            return;
        }
        self.buffer = Some(upload_gpu_buffer(device, &pack_disabled_gpu_words()));
        self.key.clear();
        self.key.extend_from_slice(DISABLED_KEY);
    }

    /// The buffer to bind at `OPTICS_TABLES_BINDING`; `None` until one was built.
    #[must_use]
    pub const fn buffer(&self) -> Option<&wgpu::Buffer> {
        self.buffer.as_ref()
    }
}

/// Whether `device` can bind the table buffer beside the storage buffers of the largest kernel.
///
/// The megakernel binds [`MEGAKERNEL_STORAGE_BUFFERS`] storage buffers, the table buffer is one
/// more, and an adaptive pass adds its pixel list: a device that grants fewer runs without the
/// tables instead of failing to build a pipeline.
#[must_use]
pub fn tables_supported_by(device: &wgpu::Device) -> bool {
    device.limits().max_storage_buffers_per_shader_stage > MEGAKERNEL_STORAGE_BUFFERS + 1
}

impl GpuFrameRenderer {
    /// `class` as this renderer can run it: without [`material_class::OPTICS_TABLES`] when the
    /// legacy wavefront is selected, the device grants too few storage buffers, or the shipped
    /// megakernel has no table variant. A class without the bit is returned unchanged.
    pub(super) fn effective_class(&self, class: u32) -> u32 {
        // Warns once per process that the tables are dropped.
        static WARNED: Once = Once::new();
        if !material_class::uses_optics_tables(class) {
            return class;
        }
        if self.pipeline_kind == GpuPipelineKind::Megakernel
            && tables_supported_by(&self.ctx.device)
            && megakernel_tables_source().is_some()
        {
            return class;
        }
        WARNED.call_once(|| {
            tracing::warn!(
                "the optics lookup tables are not available on this GPU renderer (legacy \
                 wavefront selected, too few storage buffers, or no table variant of the \
                 shader); rendering without them"
            );
        });
        class & !material_class::OPTICS_TABLES
    }

    /// Builds the table buffer of `material` when the prepared `class` uses the tables, and
    /// records whether it does ([`Self::optics_tables_in_use`]).
    pub(super) fn prepare_tables(
        &mut self,
        class: u32,
        material: &GemMaterial,
        encoded: &GpuGemMaterial,
    ) {
        self.tables_in_use = megakernel_uses_tables(class);
        if self.tables_in_use {
            self.optics_tables
                .ensure(&self.ctx.device, material, bytemuck::bytes_of(encoded));
        }
    }

    /// The table buffer a bind group of `class` must hold at `OPTICS_TABLES_BINDING`; `None` for
    /// a class that does not use the tables.
    pub(super) fn optics_tables_for(&self, class: u32) -> Option<&wgpu::Buffer> {
        if megakernel_uses_tables(class) {
            self.optics_tables.buffer()
        } else {
            None
        }
    }

    /// Whether the most recently prepared turn compiled and bound the optics lookup tables.
    ///
    /// `false` before the first turn, and for every turn whose class does not carry
    /// [`material_class::OPTICS_TABLES`] (or whose renderer cannot run the tables, see
    /// [`Self::effective_class`]).
    #[must_use]
    pub const fn optics_tables_in_use(&self) -> bool {
        self.tables_in_use
    }
}
