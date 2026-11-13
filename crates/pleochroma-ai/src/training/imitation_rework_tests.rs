//! Regressions for robust component loss and unsupported direction targets.

use super::super::burn_model::host_scalar;
use super::*;

#[test]
fn component_regression_uses_huber_while_raw_mse_remains_available() {
    let device = Device::flex();
    // The data carries the rank-3 shape it is read into (a rank-1 shape would be refused by
    // `Tensor::<3>::from_data` before any reshape).
    let hat = Tensor::<3>::from_data(TensorData::new(vec![10.0f32, 0.0], [1, 1, 2]), &device);
    let target = Tensor::<3>::zeros([1, 1, 2], &device);
    let mask = Tensor::<3>::from_data(TensorData::new(vec![1.0f32, 0.0], [1, 1, 2]), &device);
    let huber = host_scalar(masked_sens_huber(hat.clone(), target.clone(), mask.clone())).unwrap();
    let mse = host_scalar(masked_sens_mse(hat, target, mask)).unwrap();
    assert!((huber - 9.5).abs() < 1e-6, "{huber}");
    assert_eq!(mse, 100.0);
}

#[test]
fn sensitivity_huber_averages_per_sample_not_per_entry() {
    let device = Device::flex();
    // Sample 0 has three labelled entries, all with zero error; sample 1 has one labelled entry
    // with error 10 (Huber = 9.5 in its linear region). Per-sample averaging gives
    // (0 + 9.5) / 2 = 4.75. A per-entry average would give 9.5 / 4 = 2.375, which weighs the
    // small design less than the large one.
    let hat = Tensor::<3>::from_data(
        TensorData::new(vec![0.0f32, 0.0, 0.0, 10.0, 0.0, 0.0], [2, 1, 3]),
        &device,
    );
    let target = Tensor::<3>::zeros([2, 1, 3], &device);
    let mask = Tensor::<3>::from_data(
        TensorData::new(vec![1.0f32, 1.0, 1.0, 1.0, 0.0, 0.0], [2, 1, 3]),
        &device,
    );
    let huber = host_scalar(masked_sens_huber(hat, target, mask)).unwrap();
    assert!((huber - 4.75).abs() < 1e-6, "{huber}");
}

#[test]
fn zero_norm_direction_targets_do_not_add_irreducible_cosine_loss() {
    let device = Device::flex();
    let hat = Tensor::<2>::from_data(
        TensorData::new(vec![8.0f32, 8.0, 1.0, 0.0], [2, 2]),
        &device,
    );
    let target = Tensor::<2>::from_data(
        TensorData::new(vec![0.0f32, 0.0, 1.0, 0.0], [2, 2]),
        &device,
    );
    let mask = Tensor::<2>::ones([2, 2], &device);
    let (cosine, loss) = masked_cosine(hat, target, mask);
    assert!((host_scalar(cosine).unwrap() - 1.0).abs() < 1e-6);
    assert!(host_scalar(loss).unwrap().abs() < 1e-6);
}
