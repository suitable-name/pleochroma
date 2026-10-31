//! The RADV scheduler workaround switch of the worker: `--radv-workaround <on|off>` (any
//! command) and the environment variable `PLEOCHROMA_RADV_WORKAROUND`.
//!
//! Mesa's RADV driver (the open AMD Vulkan driver) mis-schedules the GPU megakernel, so some
//! zoned stones and stones with oriented inclusions render wrong; `ACO_DEBUG=nosched` makes
//! the driver correct at about 10 % lower GPU speed (`pleochroma::renderer::gpu_driver`
//! explains it and does the environment step). A headless worker has nobody to ask, so the
//! switch has a default: ON on a Linux machine with an AMD GPU on the `amdgpu` kernel driver
//! (`gpu_driver::amd_gpu_present`, read from sysfs before the GPU is touched), because a worker
//! that renders wrong pictures is worse than one that is 10 % slower; OFF everywhere else, so
//! an NVIDIA or Intel server is left exactly as it is. A user-set `ACO_DEBUG` always wins.
//! Only a `gpu` build acts on it.
//!
//! This file is the pure part (parsing, no GPU or `pleochroma` types); `main` applies it.

use std::ffi::OsStr;

/// The command-line flag.
pub const FLAG: &str = "--radv-workaround";

/// The environment variable (`on` or `off`); the flag wins over it.
pub const ENV: &str = "PLEOCHROMA_RADV_WORKAROUND";

/// Reads `on` / `off` (also `true`/`false`, `1`/`0`, `yes`/`no`; any case).
fn parse_switch(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Some(true),
        "off" | "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// Removes every [`FLAG`] from `args` and says whether the workaround is wanted.
///
/// The flag is `--radv-workaround on` or `--radv-workaround=on`; the last flag wins, then `env_value` (the value of
/// [`ENV`]). `None` when neither is given: the caller then decides by the hardware.
///
/// # Errors
///
/// A message when the flag has no value or a value other than `on`/`off`, or when the
/// environment variable holds one.
pub fn take_setting(
    args: &mut Vec<String>,
    env_value: Option<&OsStr>,
) -> Result<Option<bool>, String> {
    let mut from_flag = None;
    let mut index = 0;
    while index < args.len() {
        let value = if args[index] == FLAG {
            args.remove(index);
            if index >= args.len() {
                return Err(format!("{FLAG} needs a value: on or off"));
            }
            args.remove(index)
        } else if let Some(value) = args[index].strip_prefix(&format!("{FLAG}=")) {
            let value = value.to_owned();
            args.remove(index);
            value
        } else {
            index += 1;
            continue;
        };
        from_flag = Some(
            parse_switch(&value).ok_or_else(|| format!("{FLAG} takes on or off, not {value:?}"))?,
        );
    }
    if from_flag.is_some() {
        return Ok(from_flag);
    }
    env_value
        .map(|text| {
            parse_switch(&text.to_string_lossy())
                .ok_or_else(|| format!("{ENV} takes on or off, not {:?}", text.to_string_lossy()))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn without_flag_or_variable_the_hardware_decides() {
        let mut args = argv(&["serve", "--render"]);
        assert_eq!(take_setting(&mut args, None), Ok(None));
        assert_eq!(
            args,
            argv(&["serve", "--render"]),
            "nothing else is touched"
        );
    }

    #[test]
    fn the_flag_is_removed_in_both_spellings_and_the_last_one_wins() {
        let mut args = argv(&[
            "--radv-workaround",
            "off",
            "serve",
            "--radv-workaround=on",
            "x",
        ]);
        assert_eq!(take_setting(&mut args, None), Ok(Some(true)));
        assert_eq!(args, argv(&["serve", "x"]));

        let mut args = argv(&["join", "--radv-workaround=OFF"]);
        assert_eq!(take_setting(&mut args, None), Ok(Some(false)));
        assert_eq!(args, argv(&["join"]));
    }

    #[test]
    fn the_flag_beats_the_environment_which_beats_the_hardware() {
        let off = OsStr::new("off");
        let mut args = argv(&["serve"]);
        assert_eq!(take_setting(&mut args, Some(off)), Ok(Some(false)));
        let mut args = argv(&["--radv-workaround", "on"]);
        assert_eq!(take_setting(&mut args, Some(off)), Ok(Some(true)));
        let mut args = argv(&[]);
        assert_eq!(
            take_setting(&mut args, Some(OsStr::new("1"))),
            Ok(Some(true))
        );
    }

    #[test]
    fn a_bad_value_is_an_error_not_a_guess() {
        assert!(take_setting(&mut argv(&["--radv-workaround"]), None).is_err());
        assert!(take_setting(&mut argv(&["--radv-workaround", "maybe"]), None).is_err());
        assert!(take_setting(&mut argv(&["--radv-workaround="]), None).is_err());
        assert!(take_setting(&mut argv(&[]), Some(OsStr::new("sometimes"))).is_err());
    }
}
