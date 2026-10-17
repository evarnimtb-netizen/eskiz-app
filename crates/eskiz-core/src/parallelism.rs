//! How many worker threads to use. Phones have big.LITTLE CPUs that throttle quickly, so
//! Android caps every worker pool at [`ANDROID_THREAD_CAP`]; desktop is unchanged.

/// The most worker threads Android uses.
pub const ANDROID_THREAD_CAP: usize = 4;

/// `available` CPUs limited for a platform: `android` caps at [`ANDROID_THREAD_CAP`], otherwise
/// the count is returned as is (and never below 1).
pub fn cap_for(available: usize, android: bool) -> usize {
    let n = available.max(1);
    if android {
        n.min(ANDROID_THREAD_CAP)
    } else {
        n
    }
}

/// The CPUs this process may use, capped on Android. Replaces `available_parallelism` for
/// worker pools.
pub fn cpus() -> usize {
    cap_for(std::thread::available_parallelism().map_or(1, usize::from), cfg!(target_os = "android"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_caps_at_four_and_desktop_does_not() {
        assert_eq!(cap_for(8, true), 4);
        assert_eq!(cap_for(10, true), 4);
        assert_eq!(cap_for(2, true), 2);
        assert_eq!(cap_for(0, true), 1);
        assert_eq!(cap_for(32, false), 32);
        assert_eq!(cap_for(0, false), 1);
    }

    #[test]
    fn cpus_is_at_least_one_and_matches_this_platform() {
        let n = cpus();
        assert!(n >= 1);
        if !cfg!(target_os = "android") {
            assert_eq!(n, std::thread::available_parallelism().map_or(1, usize::from));
        }
    }
}
