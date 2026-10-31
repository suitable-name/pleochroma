//! Restore Adam tensors on the selected device, independent of Burn's default backend.
//!
//! Burn 0.22's `ModuleOptimizer::load_record` passes `Device::default()` to the state
//! loader. Its default may be CUDA even for an explicitly selected CPU run. This
//! adapter redirects only state reconstruction; every update remains Burn's Adam.

use burn::{
    module::ParamGroup,
    optim::{
        Adam, AdamConfig, DynOptimizer, DynState, LearningRate, ModuleOptimizer, StateSink,
        StateSource,
    },
    tensor::{Device, kind::BridgeTensor},
};

struct RestoreOnDevice {
    adam: Adam,
    device: Device,
}

pub(super) fn optimizer_for_restore(device: &Device) -> ModuleOptimizer {
    AdamConfig::new().init().with_group(
        ParamGroup::all(),
        RestoreOnDevice {
            adam: AdamConfig::new().build(),
            device: device.clone(),
        },
        None,
    )
}

impl DynOptimizer for RestoreOnDevice {
    fn step_dyn(
        &self,
        rank: usize,
        lr: LearningRate,
        tensor: BridgeTensor,
        grad: BridgeTensor,
        state: Option<DynState>,
    ) -> (BridgeTensor, Option<DynState>) {
        self.adam.step_dyn(rank, lr, tensor, grad, state)
    }

    fn to_device_dyn(&self, state: DynState, device: &Device) -> DynState {
        self.adam.to_device_dyn(state, device)
    }

    fn state_flatten(&self, prefix: &str, state: &DynState, out: &mut StateSink) {
        self.adam.state_flatten(prefix, state, out);
    }

    fn state_unflatten(
        &self,
        rank: usize,
        prefix: &str,
        src: &mut StateSource,
        _default_device: &Device,
    ) -> Option<DynState> {
        self.adam.state_unflatten(rank, prefix, src, &self.device)
    }
}
